use std::fmt;

use rand_core::{OsRng, RngCore};

use crate::{AppUserId, SessionId, TokenFamilyId};

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
            let replacement = generate_credential();
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
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        use fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    RefreshCredential(encoded)
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
}
