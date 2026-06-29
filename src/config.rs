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
        let v = serde_json::to_value(raw).expect("Config -> Value");
        let exchange = parse_exchange(&v)?;
        let enforce = parse_enforce(&v);
        let mode = Mode::parse(v.get("mode").and_then(|x| x.as_str()).unwrap_or("enforce"))?;
        let fail_open_on_pin_unavailable = v
            .get("failOpen")
            .and_then(|f| f.get("onPinUnavailable"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false);
        Ok(Self { exchange, enforce, mode, fail_open_on_pin_unavailable })
    }
}

fn parse_exchange(v: &serde_json::Value) -> Result<ExchangeRef, ConfigError> {
    let e = v.get("exchange").ok_or(ConfigError::MissingField("exchange"))?;
    Ok(ExchangeRef {
        org_id: required_string(e, "orgId")?,
        group_id: required_string(e, "groupId")?,
        asset_id: required_string(e, "assetId")?,
        version: required_string(e, "version")?,
        base_url: e
            .get("baseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("https://anypoint.mulesoft.com")
            .to_string(),
        auth_type: AuthType::parse(
            e.get("authType")
                .and_then(|x| x.as_str())
                .unwrap_or("oauth2_client_credentials"),
        )?,
        cred_secret_ref: required_string(e, "credSecretRef")?,
        refresh_interval_secs: e
            .get("refreshIntervalSec")
            .and_then(|x| x.as_i64())
            .unwrap_or(300)
            .clamp(30, 86_400) as u32,
    })
}

fn parse_enforce(v: &serde_json::Value) -> EnforceConfig {
    let e = v.get("enforce");
    EnforceConfig {
        exact_match: e
            .and_then(|x| x.get("exactMatch"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
        allow_added_tools: e
            .and_then(|x| x.get("allowAddedTools"))
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
        allow_removed_tools: e
            .and_then(|x| x.get("allowRemovedTools"))
            .and_then(|x| x.as_bool())
            .unwrap_or(true),
    }
}

fn required_string(v: &serde_json::Value, field: &'static str) -> Result<String, ConfigError> {
    v.get(field)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or(ConfigError::MissingField(field))
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
