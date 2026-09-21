use std::{env, error::Error, fmt, net::SocketAddr};

pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8080";
const BIND_ADDRESS_VARIABLE: &str = "COLLECTIONOPS_BIND";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppConfig {
    bind_address: SocketAddr,
}

impl AppConfig {
    /// Loads backend configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError`] when `COLLECTIONOPS_BIND` is not a valid socket address.
    pub fn from_env() -> Result<Self, ConfigError> {
        let bind_address =
            env::var(BIND_ADDRESS_VARIABLE).unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned());
        Self::from_bind_address(&bind_address)
    }

    /// Returns the address on which the HTTP server must listen.
    #[must_use]
    pub const fn bind_address(self) -> SocketAddr {
        self.bind_address
    }

    fn from_bind_address(value: &str) -> Result<Self, ConfigError> {
        value
            .parse()
            .map(|bind_address| Self { bind_address })
            .map_err(|source| ConfigError {
                variable: BIND_ADDRESS_VARIABLE,
                value: value.to_owned(),
                source,
            })
    }
}

#[derive(Debug)]
pub struct ConfigError {
    variable: &'static str,
    value: String,
    source: std::net::AddrParseError,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} must be a valid socket address, got {:?}",
            self.variable, self.value
        )
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, DEFAULT_BIND_ADDRESS};

    #[test]
    fn default_bind_address_is_valid() {
        let config = AppConfig::from_bind_address(DEFAULT_BIND_ADDRESS)
            .expect("the documented default address must be valid");

        assert_eq!(config.bind_address().to_string(), DEFAULT_BIND_ADDRESS);
    }

    #[test]
    fn invalid_bind_address_returns_actionable_error() {
        let error = AppConfig::from_bind_address("not-an-address")
            .expect_err("an invalid address must be rejected");

        assert!(error.to_string().contains("COLLECTIONOPS_BIND"));
        assert!(error.to_string().contains("not-an-address"));
    }
}
