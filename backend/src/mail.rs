use std::{env, error::Error, fmt, time::Duration};

use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor, message::Mailbox,
    transport::smtp::authentication::Credentials,
};
use url::{Url, form_urlencoded};

/// SMTP settings are server-side only. The password and invitation link are never logged.
#[derive(Clone)]
pub struct SmtpDelivery {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    public_url: Url,
}

impl fmt::Debug for SmtpDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SmtpDelivery([REDACTED])")
    }
}

#[derive(Debug)]
pub enum MailConfigError {
    Incomplete,
    InvalidPort,
    InvalidSender,
    InvalidRelay,
    InvalidPublicUrl,
}

impl fmt::Display for MailConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Incomplete => "SMTP requires COLLECTIONOPS_SMTP_HOST, COLLECTIONOPS_SMTP_USERNAME, COLLECTIONOPS_SMTP_PASSWORD, COLLECTIONOPS_SMTP_FROM and COLLECTIONOPS_PUBLIC_URL",
            Self::InvalidPort => "COLLECTIONOPS_SMTP_PORT must be a TCP port number",
            Self::InvalidSender => "COLLECTIONOPS_SMTP_FROM must be a valid mailbox",
            Self::InvalidRelay => "COLLECTIONOPS_SMTP_HOST must be a valid STARTTLS relay hostname",
            Self::InvalidPublicUrl => "COLLECTIONOPS_PUBLIC_URL must be an HTTPS server origin without credentials, path, query or fragment",
        })
    }
}
impl Error for MailConfigError {}

impl SmtpDelivery {
    /// An unset group disables invitations; a partially configured group aborts startup.
    ///
    /// # Errors
    ///
    /// Returns a configuration error for missing or malformed SMTP settings.
    pub fn from_env() -> Result<Option<Self>, MailConfigError> {
        let host = env::var("COLLECTIONOPS_SMTP_HOST")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let username = env::var("COLLECTIONOPS_SMTP_USERNAME")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let password = env::var("COLLECTIONOPS_SMTP_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty());
        let from = env::var("COLLECTIONOPS_SMTP_FROM")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let public_url = env::var("COLLECTIONOPS_PUBLIC_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if host.is_none()
            && username.is_none()
            && password.is_none()
            && from.is_none()
            && public_url.is_none()
        {
            return Ok(None);
        }
        let (Some(host), Some(username), Some(password), Some(from), Some(public_url)) =
            (host, username, password, from, public_url)
        else {
            return Err(MailConfigError::Incomplete);
        };
        let public_url = Url::parse(&public_url).map_err(|_| MailConfigError::InvalidPublicUrl)?;
        if public_url.scheme() != "https"
            || public_url.host_str().is_none()
            || !public_url.username().is_empty()
            || public_url.password().is_some()
            || public_url.path() != "/"
            || public_url.query().is_some()
            || public_url.fragment().is_some()
        {
            return Err(MailConfigError::InvalidPublicUrl);
        }
        let port = env::var("COLLECTIONOPS_SMTP_PORT")
            .ok()
            .map(|s| s.parse::<u16>().map_err(|_| MailConfigError::InvalidPort))
            .transpose()?
            .unwrap_or(587);
        if port == 0 {
            return Err(MailConfigError::InvalidPort);
        }
        let from: Mailbox = from.parse().map_err(|_| MailConfigError::InvalidSender)?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
            .map_err(|_| MailConfigError::InvalidRelay)?
            .port(port)
            .credentials(Credentials::new(username, password))
            .timeout(Some(Duration::from_secs(15)))
            .build();
        Ok(Some(Self {
            transport,
            from,
            public_url,
        }))
    }

    /// Sends an invitation via the configured STARTTLS relay.
    ///
    /// # Errors
    ///
    /// Returns a message-construction or SMTP delivery error.
    pub async fn send_invitation(
        &self,
        recipient: &str,
        space_name: &str,
        link: &str,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let to: Mailbox = recipient.parse()?;
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("server", self.public_url.as_str())
            .finish();
        let bound_link = format!("{link}?{query}");
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject("Invitation CollectionOps")
            .body(format!(
                "Vous êtes invité à rejoindre l'espace « {space_name} » de CollectionOps.\n\nServeur : {}\nOuvrez le client Windows, rubrique Compte > Invitation, puis collez ce lien :\n{bound_link}\n\nCe lien est valable sept jours et ne fonctionne qu'une fois. Si vous avez déjà un compte, connectez-vous d'abord avec cette adresse e-mail. Sinon, créez votre compte depuis l'invitation.\n\nSi vous n'attendiez pas cette invitation, ignorez ce message.",
                self.public_url
            ))?;
        self.transport.send(message).await?;
        Ok(())
    }
}
