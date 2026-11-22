use std::fmt;

use rand_core::{OsRng, RngCore};

use crate::{AppUserId, SessionId, TokenFamilyId};

/// A refresh credential is 64 hexadecimal characters.
const CREDENTIAL_HEX_LENGTH: usize = 64;

/// The leading characters every credential of one refresh family shares.
///
/// A family's first credential is 32 random bytes. Each rotation keeps the
/// first sixteen and draws the other sixteen afresh, so a consumed credential
/// can still be told to belong to its family long after its own digest has
/// been forgotten. That is what lets a replay of any earlier credential, not
/// only the last one, revoke the family while the stored state stays the same
/// size however often the family rotates.
const FAMILY_TAG_HEX_LENGTH: usize = 32;

#[derive(Clone, Eq, PartialEq)]
pub struct RefreshCredential(String);

impl RefreshCredential {
    #[must_use]
    pub fn expose_for_token_response(&self) -> &str {
        &self.0
    }

    pub(crate) fn generate() -> Self {
        generate_credential()
    }

    /// A fresh credential for the family `tag` names: the tag, then sixteen
    /// new random bytes.
    pub(crate) fn in_family(tag: &str) -> Self {
        let mut encoded = String::with_capacity(CREDENTIAL_HEX_LENGTH);
        encoded.push_str(tag);
        push_random_hex(
            &mut encoded,
            (CREDENTIAL_HEX_LENGTH - FAMILY_TAG_HEX_LENGTH) / 2,
        );
        Self(encoded)
    }

    pub(crate) fn from_protected(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for RefreshCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RefreshCredential([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshFamilyState {
    Active,
    ReplayDetected,
    Revoked,
}

#[derive(Clone, Debug)]
pub struct RefreshTokenFamily {
    family_id: TokenFamilyId,
    user_id: AppUserId,
    session_id: SessionId,
    state: RefreshFamilyState,
    generation: u64,
    current_digest: [u8; 32],
    previous: Option<ConsumedRefreshCredential>,
    current_grace_credential: Option<RefreshCredential>,
    expires_at_unix_seconds: u64,
    concurrency_grace_seconds: u64,
}

#[derive(Clone, Debug)]
struct ConsumedRefreshCredential {
    digest: [u8; 32],
    consumed_at_unix_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshExchange<T> {
    pub access_token: T,
    pub refresh_credential: RefreshCredential,
    pub generation: u64,
    pub concurrent_replay: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefreshExchangeOutcome<T> {
    Rotated(RefreshExchange<T>),
    Invalid,
    FamilyReplayDetected,
}

impl RefreshTokenFamily {
    pub fn create(
        family_id: TokenFamilyId,
        user_id: AppUserId,
        session_id: SessionId,
        expires_at_unix_seconds: u64,
        concurrency_grace_seconds: u64,
    ) -> Result<(Self, RefreshCredential), RefreshError> {
        if concurrency_grace_seconds > 30 {
            return Err(RefreshError::InvalidGraceWindow);
        }
        let credential = generate_credential();
        Ok((
            Self {
                family_id,
                user_id,
                session_id,
                state: RefreshFamilyState::Active,
                generation: 0,
                current_digest: digest(&credential),
                previous: None,
                current_grace_credential: None,
                expires_at_unix_seconds,
                concurrency_grace_seconds,
            },
            credential,
        ))
    }

    pub fn exchange<T, F>(
        &mut self,
        presented: &str,
        now_unix_seconds: u64,
        mut issue_fresh_access_token: F,
    ) -> Result<RefreshExchangeOutcome<T>, RefreshError>
    where
        F: FnMut(&AppUserId, &SessionId) -> Result<T, RefreshError>,
    {
        if self.state != RefreshFamilyState::Active
            || now_unix_seconds >= self.expires_at_unix_seconds
        {
            return Ok(RefreshExchangeOutcome::Invalid);
        }
        let presented_digest = *blake3::hash(presented.as_bytes()).as_bytes();
        if presented_digest == self.current_digest {
            let replacement = family_tag(presented)
                .map_or_else(generate_credential, RefreshCredential::in_family);
            let old_digest = self.current_digest;
            self.current_digest = digest(&replacement);
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or(RefreshError::GenerationExhausted)?;
            self.previous = Some(ConsumedRefreshCredential {
                digest: old_digest,
                consumed_at_unix_seconds: now_unix_seconds,
            });
            self.current_grace_credential = Some(replacement.clone());
            return Ok(RefreshExchangeOutcome::Rotated(RefreshExchange {
                access_token: issue_fresh_access_token(&self.user_id, &self.session_id)?,
                refresh_credential: replacement,
                generation: self.generation,
                concurrent_replay: false,
            }));
        }
        if let Some(previous) = &self.previous
            && previous.digest == presented_digest
        {
            let grace_deadline = previous
                .consumed_at_unix_seconds
                .saturating_add(self.concurrency_grace_seconds);
            if now_unix_seconds <= grace_deadline {
                let replacement = self
                    .current_grace_credential
                    .clone()
                    .ok_or(RefreshError::CorruptFamily)?;
                return Ok(RefreshExchangeOutcome::Rotated(RefreshExchange {
                    access_token: issue_fresh_access_token(&self.user_id, &self.session_id)?,
                    refresh_credential: replacement,
                    generation: self.generation,
                    concurrent_replay: true,
                }));
            }
            self.state = RefreshFamilyState::ReplayDetected;
            self.current_grace_credential = None;
            return Ok(RefreshExchangeOutcome::FamilyReplayDetected);
        }
        // Neither the current credential nor the one consumed before it. If it
        // carries this family's tag it is one the family issued and consumed
        // more than one rotation ago: a replay, whatever the time since.
        let issued_earlier = family_tag(presented).is_some_and(|tag| {
            self.current_grace_credential
                .as_ref()
                .is_some_and(|current| family_tag(&current.0) == Some(tag))
        });
        if issued_earlier {
            self.state = RefreshFamilyState::ReplayDetected;
            self.current_grace_credential = None;
            return Ok(RefreshExchangeOutcome::FamilyReplayDetected);
        }
        Ok(RefreshExchangeOutcome::Invalid)
    }

    #[must_use]
    pub const fn state(&self) -> RefreshFamilyState {
        self.state
    }

    #[must_use]
    pub fn family_id(&self) -> &TokenFamilyId {
        &self.family_id
    }
}

fn generate_credential() -> RefreshCredential {
    let mut encoded = String::with_capacity(CREDENTIAL_HEX_LENGTH);
    push_random_hex(&mut encoded, CREDENTIAL_HEX_LENGTH / 2);
    RefreshCredential(encoded)
}

/// The family tag of a well-formed credential, or `None` for anything that is
/// not 64 hexadecimal characters.
pub(crate) fn family_tag(credential: &str) -> Option<&str> {
    (credential.len() == CREDENTIAL_HEX_LENGTH
        && credential.bytes().all(|byte| byte.is_ascii_hexdigit()))
    .then(|| &credential[..FAMILY_TAG_HEX_LENGTH])
}

/// A tag for a family whose credential cannot lend its own.
pub(crate) fn generate_family_tag() -> String {
    let mut encoded = String::with_capacity(FAMILY_TAG_HEX_LENGTH);
    push_random_hex(&mut encoded, FAMILY_TAG_HEX_LENGTH / 2);
    encoded
}

fn push_random_hex(encoded: &mut String, byte_count: usize) {
    let mut bytes = [0_u8; CREDENTIAL_HEX_LENGTH / 2];
    let bytes = &mut bytes[..byte_count];
    OsRng.fill_bytes(bytes);
    for byte in bytes {
        use fmt::Write;
        write!(encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
}

fn digest(credential: &RefreshCredential) -> [u8; 32] {
    *blake3::hash(credential.0.as_bytes()).as_bytes()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshError {
    InvalidGraceWindow,
    GenerationExhausted,
    CorruptFamily,
    AccessTokenIssuance,
}

impl fmt::Display for RefreshError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidGraceWindow => "refresh concurrency grace exceeds 30 seconds",
            Self::GenerationExhausted => "refresh generation is exhausted",
            Self::CorruptFamily => "refresh token family is corrupt",
            Self::AccessTokenIssuance => "fresh access-token issuance failed",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_hashes_allows_bounded_concurrency_and_revokes_on_replay() {
        let (mut family, first) = RefreshTokenFamily::create(
            TokenFamilyId::parse("fam_abcdefgh").expect("family"),
            AppUserId::parse("usr_abcdefgh").expect("user"),
            SessionId::parse("ses_abcdefgh").expect("session"),
            1_000,
            5,
        )
        .expect("family");
        let rotated = family
            .exchange(first.expose_for_token_response(), 10, |_, _| Ok("access-1"))
            .expect("exchange");
        let RefreshExchangeOutcome::Rotated(rotated) = rotated else {
            panic!("must rotate");
        };
        assert!(!rotated.concurrent_replay);
        let concurrent = family
            .exchange(first.expose_for_token_response(), 14, |_, _| Ok("access-2"))
            .expect("concurrent exchange");
        let RefreshExchangeOutcome::Rotated(concurrent) = concurrent else {
            panic!("must accept bounded concurrency");
        };
        assert!(concurrent.concurrent_replay);
        assert_eq!(concurrent.refresh_credential, rotated.refresh_credential);
        assert_ne!(concurrent.access_token, rotated.access_token);

        assert_eq!(
            family
                .exchange::<(), _>(first.expose_for_token_response(), 16, |_, _| Ok(()))
                .expect("replay"),
            RefreshExchangeOutcome::FamilyReplayDetected
        );
        assert_eq!(family.state(), RefreshFamilyState::ReplayDetected);
        assert!(!format!("{family:?}").contains(first.expose_for_token_response()));
    }

    fn family() -> (RefreshTokenFamily, RefreshCredential) {
        RefreshTokenFamily::create(
            TokenFamilyId::parse("fam_abcdefgh").expect("family"),
            AppUserId::parse("usr_abcdefgh").expect("user"),
            SessionId::parse("ses_abcdefgh").expect("session"),
            1_000_000,
            5,
        )
        .expect("family")
    }

    fn rotate(family: &mut RefreshTokenFamily, credential: &str, now: u64) -> RefreshCredential {
        match family
            .exchange(credential, now, |_, _| Ok(()))
            .expect("exchange")
        {
            RefreshExchangeOutcome::Rotated(exchange) => exchange.refresh_credential,
            other => panic!("must rotate, got {other:?}"),
        }
    }

    #[test]
    fn a_credential_consumed_more_than_one_rotation_ago_is_a_replay() {
        let (mut family, first) = family();
        let second = rotate(&mut family, first.expose_for_token_response(), 10);
        let third = rotate(&mut family, second.expose_for_token_response(), 400);
        assert_eq!(
            family
                .exchange::<(), _>(first.expose_for_token_response(), 800, |_, _| Ok(()))
                .expect("replay"),
            RefreshExchangeOutcome::FamilyReplayDetected
        );
        assert_eq!(family.state(), RefreshFamilyState::ReplayDetected);
        assert_eq!(
            family
                .exchange::<(), _>(third.expose_for_token_response(), 801, |_, _| Ok(()))
                .expect("current after replay"),
            RefreshExchangeOutcome::Invalid
        );
    }

    #[test]
    fn a_credential_the_family_never_issued_is_invalid_and_revokes_nothing() {
        let (mut family, first) = family();
        let second = rotate(&mut family, first.expose_for_token_response(), 10);
        let third = rotate(&mut family, second.expose_for_token_response(), 400);
        let stranger = generate_credential();
        for presented in [stranger.expose_for_token_response(), "not-a-credential", ""] {
            assert_eq!(
                family
                    .exchange::<(), _>(presented, 800, |_, _| Ok(()))
                    .expect("unknown credential"),
                RefreshExchangeOutcome::Invalid
            );
        }
        assert_eq!(family.state(), RefreshFamilyState::Active);
        rotate(&mut family, third.expose_for_token_response(), 801);
    }

    #[test]
    fn rotation_keeps_the_family_tag_and_renews_the_rest() {
        let (mut family, first) = family();
        let second = rotate(&mut family, first.expose_for_token_response(), 10);
        let third = rotate(&mut family, second.expose_for_token_response(), 400);
        let tag = family_tag(first.expose_for_token_response()).expect("tag");
        assert_eq!(tag.len(), FAMILY_TAG_HEX_LENGTH);
        for credential in [&second, &third] {
            let text = credential.expose_for_token_response();
            assert_eq!(text.len(), CREDENTIAL_HEX_LENGTH);
            assert_eq!(family_tag(text), Some(tag));
        }
        let secrets = [&first, &second, &third]
            .map(|credential| &credential.expose_for_token_response()[FAMILY_TAG_HEX_LENGTH..]);
        assert_ne!(secrets[0], secrets[1]);
        assert_ne!(secrets[1], secrets[2]);
        assert_ne!(secrets[0], secrets[2]);

        assert_eq!(family_tag("abc"), None);
        assert_eq!(family_tag(&"g".repeat(CREDENTIAL_HEX_LENGTH)), None);
        let fresh = generate_family_tag();
        assert_eq!(fresh.len(), FAMILY_TAG_HEX_LENGTH);
        let minted = RefreshCredential::in_family(&fresh);
        assert_eq!(
            family_tag(minted.expose_for_token_response()),
            Some(fresh.as_str())
        );
    }
}
