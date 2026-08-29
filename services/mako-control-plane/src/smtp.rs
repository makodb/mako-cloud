use std::{error::Error, fmt, sync::Arc, time::Duration};

use async_trait::async_trait;
use lettre::{
    Message, SmtpTransport, Transport,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::{Credentials, Mechanism},
};
use mako_control_plane::{
    DeveloperMailEnvelope, DeveloperMailFailureKind, DeveloperMailTransport,
    DeveloperMailTransportError,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SmtpTlsMode {
    Wrapper,
    StartTls,
    /// No TLS at all: for a loopback relay such as mailpit in local
    /// deployments. Configuration refuses it in production before this type
    /// is ever built.
    Plaintext,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct SmtpCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ProductionSmtpConfig {
    pub relay_hostname: String,
    pub port: u16,
    pub tls_mode: SmtpTlsMode,
    /// Required unless the TLS mode is plaintext.
    pub credentials: Option<SmtpCredentials>,
    pub sender: String,
    pub timeout: Duration,
}

impl fmt::Debug for ProductionSmtpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionSmtpConfig")
            .field("relay_hostname", &self.relay_hostname)
            .field("port", &self.port)
            .field("tls_mode", &self.tls_mode)
            .field("sender", &self.sender)
            .field("timeout", &self.timeout)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct ProductionSmtpTransport {
    transport: Arc<SmtpTransport>,
    sender: Mailbox,
    sender_domain: String,
}

impl fmt::Debug for ProductionSmtpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProductionSmtpTransport")
            .field("sender_domain", &self.sender_domain)
            .finish_non_exhaustive()
    }
}

impl ProductionSmtpTransport {
    pub(crate) fn new(config: ProductionSmtpConfig) -> Result<Self, SmtpConfigurationError> {
        if config.relay_hostname.is_empty()
            || config.relay_hostname.len() > 253
            || config
                .relay_hostname
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')))
            || config.port == 0
            || !(1..=120).contains(&config.timeout.as_secs())
        {
            return Err(SmtpConfigurationError);
        }
        match &config.credentials {
            Some(credentials) => {
                if !(1..=512).contains(&credentials.username.len())
                    || credentials.username.chars().any(char::is_control)
                    || !(16..=4_096).contains(&credentials.password.len())
                    || credentials.password.chars().any(char::is_control)
                {
                    return Err(SmtpConfigurationError);
                }
            }
            None if config.tls_mode != SmtpTlsMode::Plaintext => {
                return Err(SmtpConfigurationError);
            }
            None => {}
        }
        let sender = config
            .sender
            .parse::<Mailbox>()
            .map_err(|_| SmtpConfigurationError)?;
        let sender_address = sender.email.to_string();
        let sender_domain = sender_address
            .as_str()
            .split_once('@')
            .map(|(_, domain)| domain.to_owned())
            .filter(|domain| !domain.is_empty() && domain.len() <= 253)
            .ok_or(SmtpConfigurationError)?;
        let builder = match config.tls_mode {
            SmtpTlsMode::Wrapper => SmtpTransport::relay(&config.relay_hostname),
            SmtpTlsMode::StartTls => SmtpTransport::starttls_relay(&config.relay_hostname),
            // lettre names this builder for what it is: no TLS is negotiated.
            SmtpTlsMode::Plaintext => Ok(SmtpTransport::builder_dangerous(&config.relay_hostname)),
        }
        .map_err(|_| SmtpConfigurationError)?;
        let mut builder = builder.port(config.port).timeout(Some(config.timeout));
        if let Some(credentials) = config.credentials {
            builder = builder
                .credentials(Credentials::new(credentials.username, credentials.password))
                .authentication(vec![Mechanism::Plain, Mechanism::Login]);
        }
        let transport = builder.build();
        Ok(Self {
            transport: Arc::new(transport),
            sender,
            sender_domain,
        })
    }

    fn message(
        &self,
        delivery_id: &str,
        envelope: &DeveloperMailEnvelope,
    ) -> Result<Message, DeveloperMailTransportError> {
        if !(8..=128).contains(&delivery_id.len())
            || !delivery_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(permanent("invalid_delivery_id"));
        }
        let recipient = envelope
            .recipient()
            .as_str()
            .parse::<Mailbox>()
            .map_err(|_| permanent("invalid_recipient"))?;
        Message::builder()
            .from(self.sender.clone())
            .to(recipient)
            .message_id(Some(format!("<{delivery_id}@{}>", self.sender_domain)))
            .subject(envelope.subject())
            .header(ContentType::TEXT_PLAIN)
            .body(envelope.text_body().to_owned())
            .map_err(|_| permanent("invalid_message"))
    }
}

#[async_trait]
impl DeveloperMailTransport for ProductionSmtpTransport {
    async fn readiness(&self) -> Result<(), DeveloperMailTransportError> {
        match self.transport.test_connection() {
            Ok(true) => Ok(()),
            Ok(false) => Err(DeveloperMailTransportError {
                kind: DeveloperMailFailureKind::Transient,
                stable_code: "smtp_not_ready",
            }),
            Err(error) => Err(map_smtp_error(&error)),
        }
    }

    async fn deliver(
        &self,
        delivery_id: &str,
        envelope: &DeveloperMailEnvelope,
    ) -> Result<(), DeveloperMailTransportError> {
        let message = self.message(delivery_id, envelope)?;
        self.transport
            .send(&message)
            .map(|_| ())
            .map_err(|error| map_smtp_error(&error))
    }
}

fn map_smtp_error(error: &lettre::transport::smtp::Error) -> DeveloperMailTransportError {
    if error.is_permanent() {
        permanent("smtp_permanent")
    } else if error.is_tls() {
        permanent("smtp_tls")
    } else if error.is_transient() {
        DeveloperMailTransportError {
            kind: DeveloperMailFailureKind::Transient,
            stable_code: "smtp_transient",
        }
    } else {
        DeveloperMailTransportError {
            kind: DeveloperMailFailureKind::Ambiguous,
            stable_code: if error.is_timeout() {
                "smtp_timeout"
            } else {
                "smtp_ambiguous"
            },
        }
    }
}

const fn permanent(stable_code: &'static str) -> DeveloperMailTransportError {
    DeveloperMailTransportError {
        kind: DeveloperMailFailureKind::Permanent,
        stable_code,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SmtpConfigurationError;

impl fmt::Display for SmtpConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SMTP relay configuration is invalid")
    }
}

impl Error for SmtpConfigurationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> ProductionSmtpConfig {
        ProductionSmtpConfig {
            relay_hostname: "smtp.example.test".to_owned(),
            port: 587,
            tls_mode: SmtpTlsMode::StartTls,
            credentials: Some(SmtpCredentials {
                username: "smtp-user@example.test".to_owned(),
                password: "a protected app password".to_owned(),
            }),
            sender: "Mako Cloud <no-reply@example.test>".to_owned(),
            timeout: Duration::from_secs(10),
        }
    }

    #[test]
    fn configuration_requires_credentials_tls_and_redacts_secrets() {
        let config = valid_config();
        assert!(!format!("{config:?}").contains("a protected app password"));
        assert!(!format!("{config:?}").contains("smtp-user"));
        ProductionSmtpTransport::new(config).expect("SMTP transport");
        let mut invalid = valid_config();
        invalid.credentials = Some(SmtpCredentials {
            username: "smtp-user@example.test".to_owned(),
            password: "short".to_owned(),
        });
        assert!(ProductionSmtpTransport::new(invalid).is_err());
        let mut missing = valid_config();
        missing.credentials = None;
        assert!(
            ProductionSmtpTransport::new(missing).is_err(),
            "a TLS relay without credentials is refused"
        );
    }

    #[test]
    fn plaintext_mode_builds_without_credentials_for_a_loopback_relay() {
        let mut config = valid_config();
        config.tls_mode = SmtpTlsMode::Plaintext;
        config.relay_hostname = "127.0.0.1".to_owned();
        config.port = 1025;
        config.credentials = None;
        let transport = ProductionSmtpTransport::new(config.clone()).expect("plaintext transport");
        assert_eq!(transport.sender_domain, "example.test");
        // Credentials are still accepted when the relay wants them.
        config.credentials = Some(SmtpCredentials {
            username: "mailpit".to_owned(),
            password: "a mailpit password value".to_owned(),
        });
        ProductionSmtpTransport::new(config).expect("plaintext transport with credentials");
    }

    #[test]
    fn message_builder_binds_a_deterministic_delivery_identifier() {
        let transport = ProductionSmtpTransport::new(valid_config()).expect("SMTP transport");
        let envelope = DeveloperMailEnvelope::new(
            mako_identity::NormalizedEmail::parse("person@example.test").expect("recipient"),
            "Verify your account",
            "Use the protected verification token from this message.",
        )
        .expect("envelope");
        let message = transport
            .message("dmo_delivery001", &envelope)
            .expect("message");
        let formatted = String::from_utf8(message.formatted()).expect("message bytes");
        assert!(formatted.contains("Message-ID: <dmo_delivery001@example.test>"));
        assert!(!format!("{transport:?}").contains("smtp-user"));
    }
}
