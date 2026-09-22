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
const INVITATION_TOKEN_BYTES: usize = 32;
const MAX_PASSWORD_BYTES: usize = 1024;
pub const MAX_EMAIL_BYTES: usize = 320;
const MAX_EMAIL_LOCAL_BYTES: usize = 64;
const MAX_EMAIL_DOMAIN_BYTES: usize = 255;

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

pub struct InvitationToken(String);

impl InvitationToken {
    /// Generates a 256-bit opaque invitation token using the operating system CSPRNG.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTokenError`] when the operating system random source is unavailable.
    pub fn generate() -> Result<Self, InvitationTokenError> {
        let mut bytes = [0_u8; INVITATION_TOKEN_BYTES];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| InvitationTokenError::RandomSourceUnavailable)?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// Parses a token received from an invitation link.
    ///
    /// # Errors
    ///
    /// Returns [`InvitationTokenError`] when the token is not a canonical 256-bit Base64 URL value.
    pub fn parse(value: impl Into<String>) -> Result<Self, InvitationTokenError> {
        let value = value.into();
        let bytes = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(|_| InvitationTokenError::InvalidFormat)?;

        if bytes.len() == INVITATION_TOKEN_BYTES && URL_SAFE_NO_PAD.encode(bytes) == value {
            Ok(Self(value))
        } else {
            Err(InvitationTokenError::InvalidFormat)
        }
    }

    /// Exposes the bearer secret only for delivery to the invitee.
    /// Callers must not log or persist this value.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn fingerprint(&self) -> InvitationTokenFingerprint {
        InvitationTokenFingerprint(Sha256::digest(self.0.as_bytes()).into())
    }
}

impl fmt::Debug for InvitationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvitationToken([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct InvitationTokenFingerprint([u8; 32]);

impl InvitationTokenFingerprint {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn matches(&self, candidate: &InvitationToken) -> bool {
        let candidate = candidate.fingerprint();
        bool::from(self.0.ct_eq(&candidate.0))
    }
}

impl fmt::Debug for InvitationTokenFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InvitationTokenFingerprint([REDACTED])")
    }
}

/// A normalised account e-mail address.
///
/// Normalisation is deliberately conservative: it trims the input and lowercases the domain,
/// but it does not remove dots or `+tags` from the local part, because those are provider
/// specific. The comparison policy for invitations and for the future login route still has
/// to be decided, so this type must not be used as the uniqueness rule on its own.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EmailAddress(String);

impl EmailAddress {
    /// Parses and normalises an e-mail address.
    ///
    /// # Errors
    ///
    /// Returns [`EmailAddressError`] when the value is empty, exceeds the RFC 5321 length
    /// limits, lacks exactly one non-empty local and domain part, or contains whitespace,
    /// control characters, or more than one `@`.
    pub fn parse(value: &str) -> Result<Self, EmailAddressError> {
        let trimmed = value.trim();

        if trimmed.is_empty() {
            return Err(EmailAddressError::Empty);
        }
        if trimmed.len() > MAX_EMAIL_BYTES {
            return Err(EmailAddressError::TooLong);
        }
        if trimmed.chars().any(char::is_whitespace) {
            return Err(EmailAddressError::Whitespace);
        }
        if trimmed.chars().any(char::is_control) {
            return Err(EmailAddressError::ControlCharacter);
        }

        let mut parts = trimmed.split('@');
        let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(EmailAddressError::NotExactlyOneAtSign);
        };

        if local.is_empty() || domain.is_empty() {
            return Err(EmailAddressError::MissingPart);
        }
        if local.len() > MAX_EMAIL_LOCAL_BYTES || domain.len() > MAX_EMAIL_DOMAIN_BYTES {
            return Err(EmailAddressError::TooLong);
        }
        if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
            return Err(EmailAddressError::InvalidDomain);
        }

        Ok(Self(format!("{local}@{}", domain.to_lowercase())))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EmailAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for EmailAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EmailAddress([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmailAddressError {
    Empty,
    TooLong,
    Whitespace,
    ControlCharacter,
    NotExactlyOneAtSign,
    MissingPart,
    InvalidDomain,
}

impl fmt::Display for EmailAddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "e-mail address must not be empty",
            Self::TooLong => "e-mail address exceeds the RFC 5321 length limit",
            Self::Whitespace => "e-mail address must not contain whitespace",
            Self::ControlCharacter => "e-mail address must not contain control characters",
            Self::NotExactlyOneAtSign => "e-mail address must contain exactly one @",
            Self::MissingPart => "e-mail address must have a non-empty local part and domain",
            Self::InvalidDomain => "e-mail address domain must be a dotted name",
        };
        formatter.write_str(message)
    }
}

impl Error for EmailAddressError {}

/// Validates a password supplied once at installation.
///
/// The functional password policy (minimum length, compromised-password lists, rotation) is
/// still an open decision in ADR-0003, so this only enforces the defensive input limit shared
/// with [`PasswordService::hash_password`].
///
/// # Errors
///
/// Returns [`PasswordError::EmptyPassword`] or [`PasswordError::PasswordTooLong`].
pub fn validate_bootstrap_password(password: &str) -> Result<(), PasswordError> {
    validate_password_input(password)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvitationTokenError {
    InvalidFormat,
    RandomSourceUnavailable,
}

impl fmt::Display for InvitationTokenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidFormat => "invitation token has an invalid format",
            Self::RandomSourceUnavailable => "operating system random source is unavailable",
        };
        formatter.write_str(message)
    }
}

impl Error for InvitationTokenError {}

#[cfg(test)]
mod tests {
    use super::{InvitationToken, PasswordHashValue, PasswordService, SessionToken};

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

    #[test]
    fn invitation_token_round_trips_and_only_matches_its_own_fingerprint() {
        let token = InvitationToken::generate().expect("CSPRNG must be available");
        let other = InvitationToken::generate().expect("CSPRNG must be available");
        let parsed =
            InvitationToken::parse(token.expose_secret()).expect("generated token is valid");
        let fingerprint = token.fingerprint();

        assert_eq!(token.expose_secret().len(), 43);
        assert!(fingerprint.matches(&parsed));
        assert!(!fingerprint.matches(&other));
        assert!(!format!("{token:?}").contains(token.expose_secret()));
        assert_eq!(
            format!("{fingerprint:?}"),
            "InvitationTokenFingerprint([REDACTED])"
        );
    }

    #[test]
    fn invitation_token_rejects_malformed_values() {
        assert!(InvitationToken::parse("").is_err());
        assert!(InvitationToken::parse("short").is_err());
        assert!(InvitationToken::parse("!".repeat(43)).is_err());
        let token = InvitationToken::generate().expect("CSPRNG must be available");
        assert!(InvitationToken::parse(format!("{}=", token.expose_secret())).is_err());
    }
}
