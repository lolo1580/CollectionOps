use std::{env, error::Error, fmt, net::SocketAddr};

use crate::credentials::{
    EmailAddress, EmailAddressError, PasswordError, validate_bootstrap_password,
};

pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8080";
const BIND_ADDRESS_VARIABLE: &str = "COLLECTIONOPS_BIND";
const DATABASE_URL_VARIABLE: &str = "COLLECTIONOPS_DATABASE_URL";
const BOOTSTRAP_ADMIN_EMAIL_VARIABLE: &str = "COLLECTIONOPS_BOOTSTRAP_ADMIN_EMAIL";
const BOOTSTRAP_ADMIN_PASSWORD_VARIABLE: &str = "COLLECTIONOPS_BOOTSTRAP_ADMIN_PASSWORD";
const BOOTSTRAP_ADMIN_NAME_VARIABLE: &str = "COLLECTIONOPS_BOOTSTRAP_ADMIN_NAME";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    bind_address: SocketAddr,
    database_url: Option<String>,
    bootstrap_admin: Option<BootstrapAdmin>,
}

/// The first administrator, provisioned at installation per requirement D01.
///
/// The plaintext password is held only long enough to compute its Argon2id hash; it is never
/// persisted, logged, or exposed by [`fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct BootstrapAdmin {
    email: EmailAddress,
    display_name: String,
    password: String,
}

impl AppConfig {
    /// Loads backend configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when `COLLECTIONOPS_BIND` is not a valid socket address, when the
    /// bootstrap administrator is only half configured, or when its password is unusable.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_address =
            env::var(BIND_ADDRESS_VARIABLE).unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned());
        let mut config = Self::from_bind_address(&bind_address)?;

        config.database_url = optional_value(DATABASE_URL_VARIABLE);
        config.bootstrap_admin = BootstrapAdmin::from_env()?;

        if config.bootstrap_admin.is_some() && config.database_url.is_none() {
            return Err(ConfigError::BootstrapWithoutDatabase);
        }

        Ok(config)
    }

    /// Returns the address on which the HTTP server must listen.
    #[must_use]
    pub const fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    /// Returns the MariaDB URL, or `None` when persistence is not configured.
    #[must_use]
    pub fn database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }

    /// Returns the administrator to provision at startup, when one is configured.
    #[must_use]
    pub const fn bootstrap_admin(&self) -> Option<&BootstrapAdmin> {
        self.bootstrap_admin.as_ref()
    }

    fn from_bind_address(value: &str) -> Result<Self, ConfigError> {
        value
            .parse()
            .map(|bind_address| Self {
                bind_address,
                database_url: None,
                bootstrap_admin: None,
            })
            .map_err(|source| ConfigError::BindAddress {
                value: value.to_owned(),
                source,
            })
    }
}

impl BootstrapAdmin {
    /// Reads the optional bootstrap administrator from the environment.
    ///
    /// The e-mail and the password are read together, so an operator cannot silently start
    /// without an administrator after setting only half of the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when only one of the two variables is set, when the e-mail is
    /// invalid, or when the password is empty or exceeds the defensive input limit.
    pub fn from_env() -> Result<Option<Self>, ConfigError> {
        let email = optional_value(BOOTSTRAP_ADMIN_EMAIL_VARIABLE);
        let password = optional_value(BOOTSTRAP_ADMIN_PASSWORD_VARIABLE);

        let (email, password) = match (email, password) {
            (None, None) => return Ok(None),
            (Some(_), None) => {
                return Err(ConfigError::BootstrapPartial {
                    missing: BOOTSTRAP_ADMIN_PASSWORD_VARIABLE,
                });
            }
            (None, Some(_)) => {
                return Err(ConfigError::BootstrapPartial {
                    missing: BOOTSTRAP_ADMIN_EMAIL_VARIABLE,
                });
            }
            (Some(email), Some(password)) => (email, password),
        };

        let email =
            EmailAddress::parse(&email).map_err(|source| ConfigError::BootstrapAdminEmail {
                value: email,
                source,
            })?;
        let display_name = optional_value(BOOTSTRAP_ADMIN_NAME_VARIABLE)
            .unwrap_or_else(|| email.as_str().to_owned());
        validate_bootstrap_password(&password)
            .map_err(|source| ConfigError::BootstrapAdminPassword { source })?;

        Ok(Some(Self {
            email,
            display_name,
            password,
        }))
    }

    /// Builds an administrator from explicit values, for tests and for the future installer.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when the e-mail is invalid or the password is unusable.
    pub fn from_parts(
        email: &str,
        display_name: &str,
        password: &str,
    ) -> Result<Self, ConfigError> {
        let email =
            EmailAddress::parse(email).map_err(|source| ConfigError::BootstrapAdminEmail {
                value: email.to_owned(),
                source,
            })?;
        validate_bootstrap_password(password)
            .map_err(|source| ConfigError::BootstrapAdminPassword { source })?;

        Ok(Self {
            email,
            display_name: display_name.to_owned(),
            password: password.to_owned(),
        })
    }

    #[must_use]
    pub fn email(&self) -> &EmailAddress {
        &self.email
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Exposes the password only to the component that hashes it.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for BootstrapAdmin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BootstrapAdmin")
            .field("email", &"[REDACTED]")
            .field("display_name", &self.display_name)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

fn optional_value(variable: &str) -> Option<String> {
    env::var(variable)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Debug)]
pub enum ConfigError {
    BindAddress {
        value: String,
        source: std::net::AddrParseError,
    },
    BootstrapAdminEmail {
        value: String,
        source: EmailAddressError,
    },
    BootstrapAdminPassword {
        source: PasswordError,
    },
    BootstrapPartial {
        missing: &'static str,
    },
    BootstrapWithoutDatabase,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BindAddress { value, .. } => write!(
                formatter,
                "{BIND_ADDRESS_VARIABLE} must be a valid socket address, got {value:?}"
            ),
            Self::BootstrapAdminEmail { value, source } => write!(
                formatter,
                "{BOOTSTRAP_ADMIN_EMAIL_VARIABLE} is invalid ({source}), got {value:?}"
            ),
            Self::BootstrapAdminPassword { source } => write!(
                formatter,
                "{BOOTSTRAP_ADMIN_PASSWORD_VARIABLE} is unusable ({source})"
            ),
            Self::BootstrapPartial { missing } => write!(
                formatter,
                "the bootstrap administrator requires both e-mail and password; {missing} is missing"
            ),
            Self::BootstrapWithoutDatabase => write!(
                formatter,
                "a bootstrap administrator requires {DATABASE_URL_VARIABLE} to be set"
            ),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::BindAddress { source, .. } => Some(source),
            Self::BootstrapAdminEmail { source, .. } => Some(source),
            Self::BootstrapAdminPassword { source } => Some(source),
            Self::BootstrapPartial { .. } | Self::BootstrapWithoutDatabase => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, BootstrapAdmin, DEFAULT_BIND_ADDRESS};
    use crate::credentials::EmailAddress;

    #[test]
    fn default_bind_address_is_valid() {
        let config = AppConfig::from_bind_address(DEFAULT_BIND_ADDRESS)
            .expect("the documented default address must be valid");

        assert_eq!(config.bind_address().to_string(), DEFAULT_BIND_ADDRESS);
        assert!(config.database_url().is_none());
        assert!(config.bootstrap_admin().is_none());
    }

    #[test]
    fn invalid_bind_address_returns_actionable_error() {
        let error = AppConfig::from_bind_address("not-an-address")
            .expect_err("an invalid address must be rejected");

        assert!(error.to_string().contains("COLLECTIONOPS_BIND"));
        assert!(error.to_string().contains("not-an-address"));
    }

    #[test]
    fn bootstrap_admin_never_exposes_its_password() {
        let admin = BootstrapAdmin {
            email: EmailAddress::parse("Root@Example.COM").expect("valid address"),
            display_name: "Root".to_owned(),
            password: "correct horse battery staple".to_owned(),
        };

        let debug = format!("{admin:?}");

        assert!(!debug.contains("correct horse"));
        assert!(!debug.contains("Root@Example.COM"));
        assert_eq!(admin.email().as_str(), "Root@example.com");
        assert_eq!(admin.password(), "correct horse battery staple");
    }
}
