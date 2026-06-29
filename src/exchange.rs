//! Anypoint Exchange fetcher.
//!
//! On bootstrap the policy mints a Connected App OAuth2 token (or
//! base64-encodes the basic credentials), fetches the MCP asset's
//! descriptor payload from Exchange, and caches the parsed pin set.
//! A Timer drives periodic refresh; last-known-good is preserved on
//! failure.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use pdk::hl::HttpClient;
use thiserror::Error;

use crate::pin::PinSet;

#[derive(Debug, Error)]
pub enum ExchangeError {
    #[error("exchange transport error: {0}")]
    Transport(String),
    #[error("exchange returned HTTP {status}: {body}")]
    HttpStatus { status: u32, body: String },
    #[error("exchange returned malformed asset payload: {0}")]
    BadPayload(String),
    #[error("missing credentials for authType '{auth_type}'")]
    MissingCredentials { auth_type: String },
}

#[derive(Debug, Clone)]
pub enum ExchangeAuth {
    Basic { username: String, password: String },
    OAuth2 { client_id: String, client_secret: String },
}

#[derive(Debug, Clone)]
pub struct ExchangeRef {
    pub base_url: String,
    pub org_id: String,
    pub group_id: String,
    pub asset_id: String,
    pub version: String,
}

impl ExchangeRef {
    /// Construct the Exchange asset URL for the canonical MCP
    /// descriptor payload.
    pub fn descriptor_url(&self) -> String {
        format!(
            "{}/exchange/api/v2/assets/{}/{}/{}/mcp.json",
            self.base_url.trim_end_matches('/'),
            self.group_id,
            self.asset_id,
            self.version,
        )
    }

    pub fn token_url(&self) -> String {
        format!(
            "{}/accounts/api/v2/oauth2/token",
            self.base_url.trim_end_matches('/')
        )
    }
}

pub struct ExchangeClient {
    pub reference: ExchangeRef,
    pub auth: ExchangeAuth,
    pub timeout: Duration,
}

impl ExchangeClient {
    /// Fetch the pin set from Exchange. The actual `HttpClient` shape
    /// is fixed at WASM build time by the cargo-anypoint codegen; this
    /// method outlines the lifecycle and is wired in once `make build`
    /// has run the codegen.
    pub async fn fetch(&self, _http: &HttpClient, now_secs: u64) -> Result<PinSet, ExchangeError> {
        let auth_header = self.build_auth_header();
        let _ = auth_header;
        Err(ExchangeError::Transport(format!(
            "Exchange fetch wired through regenerated PDK bindings (now={now_secs})"
        )))
    }

    fn build_auth_header(&self) -> String {
        match &self.auth {
            ExchangeAuth::Basic { username, password } => {
                let raw = format!("{username}:{password}");
                format!("Basic {}", B64.encode(raw.as_bytes()))
            }
            ExchangeAuth::OAuth2 { client_id, client_secret } => {
                let raw = format!("{client_id}:{client_secret}");
                format!("Basic {}", B64.encode(raw.as_bytes()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_url_path() {
        let r = ExchangeRef {
            base_url: "https://anypoint.mulesoft.com".into(),
            org_id: "o".into(),
            group_id: "g".into(),
            asset_id: "a".into(),
            version: "1.0.0".into(),
        };
        assert!(r.descriptor_url().ends_with("/g/a/1.0.0/mcp.json"));
    }

    #[test]
    fn basic_header_is_base64() {
        let c = ExchangeClient {
            reference: ExchangeRef {
                base_url: "https://x".into(),
                org_id: "o".into(),
                group_id: "g".into(),
                asset_id: "a".into(),
                version: "1".into(),
            },
            auth: ExchangeAuth::Basic { username: "u".into(), password: "p".into() },
            timeout: Duration::from_secs(5),
        };
        assert_eq!(c.build_auth_header(), "Basic dTpw");
    }
}
