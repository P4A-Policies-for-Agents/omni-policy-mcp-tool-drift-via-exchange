//! Typed view over the policy configuration.

use crate::generated::config::Config;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("exchange.{0} is required and must be non-empty")]
    MissingField(&'static str),
    #[error("unknown exchange.authType: {0}")]
    UnknownAuthType(String),
    #[error("unknown mode: {0}")]
    UnknownMode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Enforce,
    Observe,
    Warn,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s {
            "enforce" => Ok(Self::Enforce),
            "observe" => Ok(Self::Observe),
            "warn" => Ok(Self::Warn),
            other => Err(ConfigError::UnknownMode(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    Basic,
    OAuth2ClientCredentials,
}

impl AuthType {
    pub fn parse(s: &str) -> Result<Self, ConfigError> {
        match s {
            "basic" => Ok(Self::Basic),
            "oauth2_client_credentials" => Ok(Self::OAuth2ClientCredentials),
            other => Err(ConfigError::UnknownAuthType(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExchangeRef {
    pub org_id: String,
    pub group_id: String,
    pub asset_id: String,
    pub version: String,
    pub base_url: String,
    pub auth_type: AuthType,
    pub cred_secret_ref: String,
    pub refresh_interval_secs: u32,
    /// Optional path prefix prepended to every Exchange request path
    /// (e.g. `/exchange-pin`). When non-empty the policy runs in
    /// "loopback" mode: it dispatches the pin fetch to `service`
    /// (`baseUrl`) verbatim instead of discovering the request's own
    /// upstream cluster. See `gcl.yaml` `exchangePathPrefix`.
    pub path_prefix: String,
    /// Optional path prefix for the SECOND loopback hop that fetches the
    /// descriptor FILE content from the pre-signed storage URL (e.g.
    /// `/exchange-s3`). Selects a passthrough route whose
    /// `auto_host_rewrite` restores the S3 Host. See `gcl.yaml`
    /// `exchangeFilePathPrefix`.
    pub file_path_prefix: String,
    /// Cargo-anypoint codegen registers this handle at boot via
    /// `abi.service_create`; it is the required argument to
    /// `HttpClient::request(&service)` at runtime.
    pub service: Option<pdk::hl::Service>,
}

#[derive(Debug, Clone)]
pub struct EnforceConfig {
    pub exact_match: bool,
    pub allow_added_tools: bool,
    pub allow_removed_tools: bool,
}

#[derive(Debug, Clone)]
pub struct PolicyConfig {
    pub exchange: ExchangeRef,
    pub enforce: EnforceConfig,
    pub mode: Mode,
    pub fail_open_on_pin_unavailable: bool,
}

impl PolicyConfig {
    pub fn from_config(raw: &Config) -> Result<Self, ConfigError> {
        let exchange = parse_exchange(&raw.exchange)?;
        let enforce = parse_enforce(raw.enforce.as_ref());
        let mode = Mode::parse(raw.mode.as_deref().unwrap_or("enforce"))?;
        let fail_open_on_pin_unavailable = raw
            .fail_open
            .as_ref()
            .and_then(|f| f.on_pin_unavailable)
            .unwrap_or(false);
        Ok(Self { exchange, enforce, mode, fail_open_on_pin_unavailable })
    }
}

fn require(value: &str, field: &'static str) -> Result<String, ConfigError> {
    if value.is_empty() {
        Err(ConfigError::MissingField(field))
    } else {
        Ok(value.to_string())
    }
}

fn parse_exchange(
    e: &crate::generated::config::ExchangeConfig,
) -> Result<ExchangeRef, ConfigError> {
    let base_url = e
        .base_url
        .as_ref()
        .map(|s| s.uri().to_string())
        .unwrap_or_else(|| "https://anypoint.mulesoft.com".to_string());
    Ok(ExchangeRef {
        org_id: require(&e.org_id, "orgId")?,
        group_id: require(&e.group_id, "groupId")?,
        asset_id: require(&e.asset_id, "assetId")?,
        version: require(&e.version, "version")?,
        base_url,
        auth_type: AuthType::parse(
            e.auth_type.as_deref().unwrap_or("oauth2_client_credentials"),
        )?,
        cred_secret_ref: require(&e.cred_secret_ref, "credSecretRef")?,
        refresh_interval_secs: e
            .refresh_interval_sec
            .unwrap_or(300)
            .clamp(30, 86_400) as u32,
        path_prefix: e
            .exchange_path_prefix
            .as_deref()
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string(),
        file_path_prefix: e
            .exchange_file_path_prefix
            .as_deref()
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .to_string(),
        service: e.base_url.clone(),
    })
}

fn parse_enforce(
    e: Option<&crate::generated::config::EnforceConfig>,
) -> EnforceConfig {
    EnforceConfig {
        exact_match: e.and_then(|x| x.exact_match).unwrap_or(true),
        allow_added_tools: e.and_then(|x| x.allow_added_tools).unwrap_or(false),
        allow_removed_tools: e.and_then(|x| x.allow_removed_tools).unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_known_values() {
        assert_eq!(Mode::parse("enforce").unwrap(), Mode::Enforce);
        assert_eq!(Mode::parse("observe").unwrap(), Mode::Observe);
        assert_eq!(Mode::parse("warn").unwrap(), Mode::Warn);
        assert!(Mode::parse("yolo").is_err());
    }

    #[test]
    fn auth_type_parses_known_values() {
        assert_eq!(AuthType::parse("basic").unwrap(), AuthType::Basic);
        assert_eq!(
            AuthType::parse("oauth2_client_credentials").unwrap(),
            AuthType::OAuth2ClientCredentials
        );
        assert!(AuthType::parse("magic").is_err());
    }
}
