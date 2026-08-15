use std::{error::Error, fmt};

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use rand_core::OsRng;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordPolicy {
    minimum_characters: usize,
    maximum_bytes: usize,
    require_uppercase: bool,
    require_lowercase: bool,
    require_digit: bool,
    require_symbol: bool,
}

impl PasswordPolicy {
    pub fn new(
        minimum_characters: usize,
        maximum_bytes: usize,
        require_uppercase: bool,
        require_lowercase: bool,
        require_digit: bool,
        require_symbol: bool,
    ) -> Result<Self, PasswordError> {
        if !(8..=128).contains(&minimum_characters) {
            return Err(PasswordError::InvalidConfiguration(
                "minimum password length must be between 8 and 128 characters",
            ));
        }
        if maximum_bytes < minimum_characters || maximum_bytes > 4096 {
            return Err(PasswordError::InvalidConfiguration(
                "maximum password size must cover the minimum and not exceed 4096 bytes",
            ));
        }
        Ok(Self {
            minimum_characters,
            maximum_bytes,
            require_uppercase,
            require_lowercase,
            require_digit,
            require_symbol,
        })
    }

    pub fn validate(&self, password: &str) -> Result<(), PasswordPolicyViolation> {
        if password.len() > self.maximum_bytes {
            return Err(PasswordPolicyViolation::TooLong);
        }
        if password.chars().count() < self.minimum_characters {
            return Err(PasswordPolicyViolation::TooShort);
        }
        if self.require_uppercase && !password.chars().any(char::is_uppercase) {
            return Err(PasswordPolicyViolation::UppercaseRequired);
        }
        if self.require_lowercase && !password.chars().any(char::is_lowercase) {
            return Err(PasswordPolicyViolation::LowercaseRequired);
        }
        if self.require_digit && !password.chars().any(|character| character.is_ascii_digit()) {
            return Err(PasswordPolicyViolation::DigitRequired);
        }
        if self.require_symbol
            && !password
                .chars()
                .any(|character| !character.is_alphanumeric())
        {
            return Err(PasswordPolicyViolation::SymbolRequired);
        }
        Ok(())
    }
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self::new(12, 1024, false, false, false, false).expect("default password policy is valid")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordPolicyViolation {
    TooShort,
    TooLong,
    UppercaseRequired,
    LowercaseRequired,
    DigitRequired,
    SymbolRequired,
}

impl PasswordPolicyViolation {
    #[must_use]
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::TooShort => "password_too_short",
            Self::TooLong => "password_too_long",
            Self::UppercaseRequired => "password_uppercase_required",
            Self::LowercaseRequired => "password_lowercase_required",
            Self::DigitRequired => "password_digit_required",
            Self::SymbolRequired => "password_symbol_required",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Argon2idParameters {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_bytes: usize,
}

impl Argon2idParameters {
    pub fn new(
        memory_kib: u32,
        iterations: u32,
        parallelism: u32,
        output_bytes: usize,
    ) -> Result<Self, PasswordError> {
        Params::new(memory_kib, iterations, parallelism, Some(output_bytes))
            .map_err(PasswordError::Argon2)?;
        if memory_kib < 8 * 1024 || iterations < 2 || output_bytes < 32 {
            return Err(PasswordError::InvalidConfiguration(
                "Argon2id requires at least 8192 KiB, two iterations, and 32 output bytes",
            ));
        }
        Ok(Self {
            memory_kib,
            iterations,
            parallelism,
            output_bytes,
        })
    }

    fn params(self) -> Result<Params, PasswordError> {
        Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            Some(self.output_bytes),
        )
        .map_err(PasswordError::Argon2)
    }
}

impl Default for Argon2idParameters {
    fn default() -> Self {
        Self::new(19 * 1024, 2, 1, 32).expect("default Argon2id parameters are valid")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct StoredPasswordHash(String);

impl StoredPasswordHash {
    pub fn parse(value: impl Into<String>) -> Result<Self, PasswordError> {
        let value = value.into();
        PasswordHash::new(&value).map_err(PasswordError::PasswordHash)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn encoded(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StoredPasswordHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StoredPasswordHash([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PasswordVerification {
    Invalid,
    Valid {
        upgraded: Option<StoredPasswordHash>,
    },
}

#[derive(Clone, Debug)]
pub struct PasswordService {
    policy: PasswordPolicy,
    parameters: Argon2idParameters,
}

impl PasswordService {
    #[must_use]
    pub const fn new(policy: PasswordPolicy, parameters: Argon2idParameters) -> Self {
        Self { policy, parameters }
    }

    pub fn hash(&self, password: &str) -> Result<StoredPasswordHash, PasswordError> {
        self.policy
            .validate(password)
            .map_err(PasswordError::PolicyViolation)?;
        self.hash_without_policy_check(password)
    }

    pub fn verify(
        &self,
        password: &str,
        stored: &StoredPasswordHash,
    ) -> Result<PasswordVerification, PasswordError> {
        let parsed = PasswordHash::new(stored.encoded()).map_err(PasswordError::PasswordHash)?;
        if Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(PasswordVerification::Invalid);
        }
        let upgraded = self
            .needs_upgrade(&parsed)?
            .then(|| self.hash_without_policy_check(password))
            .transpose()?;
        Ok(PasswordVerification::Valid { upgraded })
    }

    fn hash_without_policy_check(
        &self,
        password: &str,
    ) -> Result<StoredPasswordHash, PasswordError> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::new(
            Algorithm::Argon2id,
            Version::V0x13,
            self.parameters.params()?,
        );
        let hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(PasswordError::PasswordHash)?;
        StoredPasswordHash::parse(hash.to_string())
    }

    fn needs_upgrade(&self, hash: &PasswordHash<'_>) -> Result<bool, PasswordError> {
        let params = Params::try_from(hash).map_err(PasswordError::PasswordHash)?;
        Ok(hash.algorithm.as_str() != "argon2id"
            || hash.version != Some(Version::V0x13 as u32)
            || params.m_cost() < self.parameters.memory_kib
            || params.t_cost() < self.parameters.iterations
            || params.p_cost() < self.parameters.parallelism
            || hash
                .hash
                .is_none_or(|output| output.len() < self.parameters.output_bytes))
    }
}

#[derive(Debug)]
pub enum PasswordError {
    InvalidConfiguration(&'static str),
    PolicyViolation(PasswordPolicyViolation),
    Argon2(argon2::Error),
    PasswordHash(argon2::password_hash::Error),
}

impl fmt::Display for PasswordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration(message) => formatter.write_str(message),
            Self::PolicyViolation(violation) => formatter.write_str(violation.stable_code()),
            Self::Argon2(error) => error.fmt(formatter),
            Self::PasswordHash(error) => error.fmt(formatter),
        }
    }
}

impl Error for PasswordError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_is_configurable_and_hashes_are_redacted() {
        let service = PasswordService::new(
            PasswordPolicy::new(12, 128, true, true, true, true).expect("policy"),
            Argon2idParameters::new(8 * 1024, 2, 1, 32).expect("parameters"),
        );
        assert!(matches!(
            service.hash("too-short"),
            Err(PasswordError::PolicyViolation(_))
        ));
        let hash = service.hash("Correct-Horse-7!").expect("hash");
        assert_eq!(format!("{hash:?}"), "StoredPasswordHash([REDACTED])");
        assert_eq!(
            service.verify("wrong-password", &hash).expect("verify"),
            PasswordVerification::Invalid
        );
        assert_eq!(
            service.verify("Correct-Horse-7!", &hash).expect("verify"),
            PasswordVerification::Valid { upgraded: None }
        );
    }

    #[test]
    fn valid_weaker_hash_is_upgraded_automatically() {
        let weak = PasswordService::new(
            PasswordPolicy::default(),
            Argon2idParameters::new(8 * 1024, 2, 1, 32).expect("parameters"),
        );
        let stronger = PasswordService::new(
            PasswordPolicy::default(),
            Argon2idParameters::new(8 * 1024, 3, 1, 32).expect("parameters"),
        );
        let hash = weak.hash("long-enough-password").expect("hash");
        let PasswordVerification::Valid {
            upgraded: Some(upgraded),
        } = stronger
            .verify("long-enough-password", &hash)
            .expect("verify")
        else {
            panic!("valid weaker hash must be upgraded");
        };
        assert_eq!(
            stronger
                .verify("long-enough-password", &upgraded)
                .expect("verify"),
            PasswordVerification::Valid { upgraded: None }
        );
    }
}
