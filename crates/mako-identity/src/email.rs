use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::IdentityRecordError;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NormalizedEmail(String);

impl NormalizedEmail {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, IdentityRecordError> {
        let value = value.as_ref();
        if value.trim() != value || value.len() > 254 || !value.is_ascii() {
            return Err(invalid_email());
        }
        let Some((local, domain)) = value.split_once('@') else {
            return Err(invalid_email());
        };
        if local.is_empty()
            || local.len() > 64
            || domain.is_empty()
            || domain.contains('@')
            || local.starts_with('.')
            || local.ends_with('.')
            || local.contains("..")
            || !local.bytes().all(valid_local_byte)
            || !valid_domain(domain)
        {
            return Err(invalid_email());
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedEmail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for NormalizedEmail {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NormalizedEmail {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

fn valid_local_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'.' | b'!'
                | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'/'
                | b'='
                | b'?'
                | b'^'
                | b'_'
                | b'`'
                | b'{'
                | b'|'
                | b'}'
                | b'~'
        )
}

fn valid_domain(domain: &str) -> bool {
    domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn invalid_email() -> IdentityRecordError {
    IdentityRecordError::InvalidField {
        field: "email",
        reason: "must be a valid ASCII address with a DNS-style domain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_is_stable_and_rejects_ambiguous_inputs() {
        assert_eq!(
            NormalizedEmail::parse("Person.Tag@Example.COM")
                .expect("email")
                .as_str(),
            "person.tag@example.com"
        );
        for invalid in [
            " person@example.com",
            "person@example.com ",
            "person@@example.com",
            ".person@example.com",
            "person@example",
            "person@-example.com",
        ] {
            assert!(NormalizedEmail::parse(invalid).is_err(), "{invalid}");
        }
    }
}
