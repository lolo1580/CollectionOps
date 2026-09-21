use std::{error::Error, fmt};

use argon2::{
    Algorithm, Argon2, Params, PasswordHasher, PasswordVerifier, Version,
    password_hash::{PasswordHash, SaltString, rand_core::OsRng},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::RngCore;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const ARGON2_MEMORY_KIB: u32 = 64 * 1024;
const ARGON2_ITERATIONS: u32 = 3;
const ARGON2_PARALLELISM: u32 = 4;
const ARGON2_OUTPUT_BYTES: usize = 32;
const SESSION_TOKEN_BYTES: usize = 32;
const MAX_PASSWORD_BYTES: usize = 1024;

pub struct PasswordService {
    argon2: Argon2<'static>,
}

impl Default for PasswordService {
    fn default() -> Self {
        let params = Params::new(
            ARGON2_MEMORY_KIB,
            ARGON2_ITERATIONS,
            ARGON2_PARALLELISM,
            Some(ARGON2_OUTPUT_BYTES),
        )
        .expect("the fixed Argon2id parameters must be valid");

        Self {
            argon2: Argon2::new(Algorithm::Argon2id, Version::V0x13, params),
        }
    }
}

impl PasswordService {
    /// Hashes a password with a fresh random salt and returns a PHC-formatted value.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordError`] when the password is empty, exceeds the defensive input limit,
    /// or cannot be hashed by the configured Argon2id implementation.
    pub fn hash_password(&self, password: &str) -> Result<PasswordHashValue, PasswordError> {
        validate_password_input(password)?;

        let salt = SaltString::generate(&mut OsRng);
        self.argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| PasswordHashValue(hash.to_string()))
            .map_err(|_| PasswordError::HashingFailed)
    }

    /// Verifies a password against a PHC-formatted Argon2id hash.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordError`] for an invalid password input or malformed stored hash.
    pub fn verify_password(
        &self,
        password: &str,
        expected: &PasswordHashValue,
    ) -> Result<bool, PasswordError> {
        validate_password_input(password)?;
        let parsed =
            PasswordHash::new(expected.as_str()).map_err(|_| PasswordError::InvalidStoredHash)?;

        Ok(self
            .argon2
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    }
}

fn validate_password_input(password: &str) -> Result<(), PasswordError> {
    if password.is_empty() {
        Err(PasswordError::EmptyPassword)
    } else if password.len() > MAX_PASSWORD_BYTES {
        Err(PasswordError::PasswordTooLong)
    } else {
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PasswordHashValue(String);

impl PasswordHashValue {
    /// Parses a stored PHC password hash without verifying a password.
    ///
    /// # Errors
    ///
    /// Returns [`PasswordError`] when the value is not a valid PHC string.
    pub fn parse(value: impl Into<String>) -> Result<Self, PasswordError> {
        let value = value.into();
        PasswordHash::new(&value).map_err(|_| PasswordError::InvalidStoredHash)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PasswordHashValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PasswordHashValue([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    EmptyPassword,
    PasswordTooLong,
    InvalidStoredHash,
    HashingFailed,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyPassword => "password must not be empty",
            Self::PasswordTooLong => "password exceeds the defensive input limit",
            Self::InvalidStoredHash => "stored password hash is invalid",
            Self::HashingFailed => "password hashing failed",
        };
        formatter.write_str(message)
    }
}

impl Error for PasswordError {}

pub struct SessionToken(String);

impl SessionToken {
    /// Generates a 256-bit opaque session token using the operating system CSPRNG.
    ///
    /// # Errors
    ///
    /// Returns [`SessionTokenError`] when the operating system random source is unavailable.
    pub fn generate() -> Result<Self, SessionTokenError> {
        let mut bytes = [0_u8; SESSION_TOKEN_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| SessionTokenError::RandomSourceUnavailable)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Exposes the bearer secret for transport to the authenticated client.
    ///
    /// Callers must not log, persist, or include this value in diagnostics.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn fingerprint(&self) -> SessionTokenFingerprint {
        SessionTokenFingerprint(Sha256::digest(self.0.as_bytes()).into())
    }
}

impl fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionToken([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SessionTokenFingerprint([u8; 32]);

impl SessionTokenFingerprint {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn matches(&self, candidate: &SessionToken) -> bool {
        let candidate = candidate.fingerprint();
        bool::from(self.0.ct_eq(&candidate.0))
    }
}

impl fmt::Debug for SessionTokenFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionTokenFingerprint([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTokenError {
    RandomSourceUnavailable,
}

impl fmt::Display for SessionTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("operating system random source is unavailable")
    }
}

impl Error for SessionTokenError {}

#[cfg(test)]
mod tests {
    use super::{PasswordHashValue, PasswordService, SessionToken};

    #[test]
    fn password_hash_uses_argon2id_and_verifies_only_the_original_password() {
        let service = PasswordService::default();
        let hash = service
            .hash_password("correct horse battery staple")
            .expect("password must be hashable");

        assert!(hash.as_str().starts_with("$argon2id$v=19$m=65536,t=3,p=4$"));
        assert!(
            service
                .verify_password("correct horse battery staple", &hash)
                .expect("stored hash must be valid")
        );
        assert!(
            !service
                .verify_password("wrong password", &hash)
                .expect("stored hash must be valid")
        );
        assert!(!format!("{hash:?}").contains(hash.as_str()));
    }

    #[test]
    fn malformed_password_hash_is_rejected() {
        assert!(PasswordHashValue::parse("not-a-phc-hash").is_err());
    }

    #[test]
    fn session_token_is_opaque_unique_and_redacted() {
        let first = SessionToken::generate().expect("CSPRNG must be available");
        let second = SessionToken::generate().expect("CSPRNG must be available");

        assert_eq!(first.expose_secret().len(), 43);
        assert_ne!(first.expose_secret(), second.expose_secret());
        assert!(!format!("{first:?}").contains(first.expose_secret()));
    }

    #[test]
    fn session_fingerprint_matches_only_its_token() {
        let token = SessionToken::generate().expect("CSPRNG must be available");
        let other = SessionToken::generate().expect("CSPRNG must be available");
        let fingerprint = token.fingerprint();

        assert!(fingerprint.matches(&token));
        assert!(!fingerprint.matches(&other));
    }
}
