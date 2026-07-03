//! Anypoint Exchange fetcher.
//!
//! On the first inbound request (and again after the refresh interval)
//! the policy mints a Connected App OAuth2 client-credentials token (or
//! base64-encodes Basic credentials), fetches the MCP asset's canonical
//! descriptor payload from Exchange, and parses it into a `PinSet`.
//! Last-known-good is preserved by the caller on failure.
//!
//! The real outbound call is made through the per-request PDK
//! `HttpClient` + a `pdk::hl::Service` handle (registered from the
//! `format: service` `baseUrl` config field). Outbound HTTPS only
//! connects from the request/response filter phases under connected-mode
//! Flex Gateway — never from `configure()` or a background timer — which
//! is why `fetch` takes the `HttpClient`/`Service` supplied to the
//! filter callbacks. See the reference `a2d.rs` for the same mechanics.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use pdk::hl::{HttpClient, Service};
use pdk::logger;
use thiserror::Error;

use crate::pin::PinSet;

const FETCH_TIMEOUT_SECS: u64 = 30;

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
    #[error("exchange token endpoint returned no access_token: {0}")]
    MissingToken(String),
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
    /// Prepended to every Exchange request path (e.g. `/exchange-pin`).
    /// Empty for a direct Exchange call; set when routing through a
    /// gateway loopback route that restores the correct egress `Host`.
    pub path_prefix: String,
    /// Prepended to the SECOND loopback hop that fetches the descriptor
    /// FILE content from the pre-signed storage URL (e.g. `/exchange-s3`).
    /// Selects the passthrough route whose `auto_host_rewrite` restores
    /// the storage (S3) `Host` enforced by the pre-signed URL. Empty when
    /// the gateway can reach the storage host directly.
    pub file_path_prefix: String,
}

impl ExchangeRef {
    /// Path (host-relative) of the canonical MCP descriptor payload,
    /// including the optional loopback `path_prefix`.
    pub fn descriptor_path(&self) -> String {
        format!(
            "{}/exchange/api/v2/assets/{}/{}/{}/mcp.json",
            self.path_prefix, self.group_id, self.asset_id, self.version,
        )
    }

    /// Path (host-relative) of the asset METADATA endpoint, including the
    /// optional loopback `path_prefix`. This returns the asset's JSON
    /// metadata (with the `files` array of pre-signed links) — NOT the
    /// descriptor file itself, so there is no `/mcp.json` suffix.
    pub fn metadata_path(&self) -> String {
        format!(
            "{}/exchange/api/v2/assets/{}/{}/{}",
            self.path_prefix, self.group_id, self.asset_id, self.version,
        )
    }

    /// Path (host-relative) of the OAuth2 token endpoint, including the
    /// optional loopback `path_prefix`.
    pub fn token_path(&self) -> String {
        format!("{}/accounts/api/v2/oauth2/token", self.path_prefix)
    }

    /// Build the host-relative path for the SECOND hop that fetches the
    /// descriptor file content from a pre-signed storage URL. Strips the
    /// scheme + authority from `external_link` (keeping the path and query
    /// string) and prepends `file_path_prefix` so the request re-enters
    /// through the storage passthrough route. Returns `BadPayload` if the
    /// link carries no path component.
    fn file_loopback_path(&self, external_link: &str) -> Result<String, ExchangeError> {
        let after_scheme = external_link
            .find("://")
            .map(|i| &external_link[i + 3..])
            .ok_or_else(|| {
                ExchangeError::BadPayload(format!(
                    "descriptor externalLink has no scheme: {external_link}"
                ))
            })?;
        let slash = after_scheme.find('/').ok_or_else(|| {
            ExchangeError::BadPayload(format!(
                "descriptor externalLink has no path: {external_link}"
            ))
        })?;
        let path_and_query = &after_scheme[slash..];
        Ok(format!("{}{}", self.file_path_prefix, path_and_query))
    }

    /// Absolute descriptor URL (for logging / display).
    pub fn descriptor_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.descriptor_path())
    }

    /// Absolute token URL (for logging / display).
    pub fn token_url(&self) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.token_path())
    }
}

pub struct ExchangeClient {
    pub reference: ExchangeRef,
    pub auth: ExchangeAuth,
    pub timeout: Duration,
}

impl ExchangeClient {
    pub fn new(reference: ExchangeRef, auth: ExchangeAuth) -> Self {
        Self {
            reference,
            auth,
            timeout: Duration::from_secs(FETCH_TIMEOUT_SECS),
        }
    }

    /// Fetch the pin set from Exchange via the real two-hop asset flow.
    ///
    /// For `oauth2_client_credentials` this first POSTs to the token
    /// endpoint to mint a bearer token; for `basic` it sends the Basic
    /// header directly. Then:
    ///   1. GET the asset METADATA (`metadata_path()`) with Authorization
    ///      and select the descriptor file's pre-signed `externalLink`.
    ///   2. GET that file through the SECOND loopback hop
    ///      (`file_loopback_path()`) with NO Authorization (the URL is
    ///      pre-signed) and parse the body into a `PinSet`.
    /// Both hops use the SAME `service` (`baseUrl`); only the path prefix
    /// differs. Errors on missing credentials, transport failure, non-2xx
    /// status, or a malformed payload.
    pub async fn fetch(
        &self,
        http: &HttpClient,
        service: &Service,
        now_secs: u64,
    ) -> Result<PinSet, ExchangeError> {
        self.ensure_credentials()?;

        let authority = service.uri().authority().to_string();
        let authorization = self.authorization(http, service, &authority).await?;

        // Hop 1: fetch asset metadata (JSON with a `files` array of
        // pre-signed links) with Authorization.
        let metadata_path = self.reference.metadata_path();
        let metadata_headers = vec![
            ("host", authority.as_str()),
            ("accept", "application/json"),
            ("authorization", authorization.as_str()),
        ];

        logger::debug!(
            "mcp-drift-exchange: fetching asset metadata authority={} path={}",
            authority,
            metadata_path
        );

        let metadata_response = http
            .request(service)
            .path(metadata_path.as_str())
            .timeout(self.timeout)
            .headers(metadata_headers)
            .get()
            .await
            .map_err(|e| ExchangeError::Transport(format!("metadata: {:?}", e)))?;

        let metadata_status = metadata_response.status_code();
        if !(200..300).contains(&metadata_status) {
            return Err(ExchangeError::HttpStatus {
                status: metadata_status,
                body: String::from_utf8_lossy(metadata_response.body()).to_string(),
            });
        }

        // Select the descriptor file's pre-signed link, then build the
        // second-hop path that re-enters through the storage route.
        let external_link = parse_metadata_for_descriptor_link(metadata_response.body())?;
        let file_path = self.reference.file_loopback_path(&external_link)?;
        let file_headers = vec![("host", authority.as_str()), ("accept", "*/*")];

        logger::debug!(
            "mcp-drift-exchange: fetching descriptor file authority={} path={}",
            authority,
            file_path
        );

        // Hop 2: fetch the descriptor file content. NO Authorization —
        // the pre-signed URL already authenticates the request.
        let file_response = http
            .request(service)
            .path(file_path.as_str())
            .timeout(self.timeout)
            .headers(file_headers)
            .get()
            .await
            .map_err(|e| ExchangeError::Transport(format!("file: {:?}", e)))?;

        let file_status = file_response.status_code();
        if !(200..300).contains(&file_status) {
            return Err(ExchangeError::HttpStatus {
                status: file_status,
                body: String::from_utf8_lossy(file_response.body()).to_string(),
            });
        }

        parse_descriptor(file_response.body(), now_secs, &self.reference.version)
    }

    /// Resolve the `Authorization` header value for the descriptor GET.
    /// Basic auth is sent verbatim; OAuth2 client-credentials is
    /// exchanged for a bearer token at the token endpoint first.
    async fn authorization(
        &self,
        http: &HttpClient,
        service: &Service,
        authority: &str,
    ) -> Result<String, ExchangeError> {
        match &self.auth {
            ExchangeAuth::Basic { .. } => Ok(self.build_auth_header()),
            ExchangeAuth::OAuth2 { client_id, client_secret } => {
                let token = self
                    .mint_token(http, service, authority, client_id, client_secret)
                    .await?;
                Ok(format!("Bearer {token}"))
            }
        }
    }

    /// POST `grant_type=client_credentials` to the token endpoint and
    /// return the `access_token`. The client id/secret are sent both in
    /// the form body and as a Basic `Authorization` header so the
    /// endpoint accepts either binding.
    async fn mint_token(
        &self,
        http: &HttpClient,
        service: &Service,
        authority: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, ExchangeError> {
        let path = self.reference.token_path();
        let basic = format!(
            "Basic {}",
            B64.encode(format!("{client_id}:{client_secret}").as_bytes())
        );
        let form = format!(
            "grant_type=client_credentials&client_id={}&client_secret={}",
            urlencode(client_id),
            urlencode(client_secret),
        );
        let headers = vec![
            ("host", authority),
            ("accept", "application/json"),
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", basic.as_str()),
        ];

        let response = http
            .request(service)
            .path(path.as_str())
            .timeout(self.timeout)
            .headers(headers)
            .body(form.as_bytes())
            .post()
            .await
            .map_err(|e| ExchangeError::Transport(format!("token: {:?}", e)))?;

        let status = response.status_code();
        if !(200..300).contains(&status) {
            return Err(ExchangeError::HttpStatus {
                status,
                body: String::from_utf8_lossy(response.body()).to_string(),
            });
        }

        parse_access_token(response.body())
    }

    fn ensure_credentials(&self) -> Result<(), ExchangeError> {
        let missing = match &self.auth {
            ExchangeAuth::Basic { username, password } => {
                username.is_empty() && password.is_empty()
            }
            ExchangeAuth::OAuth2 { client_id, client_secret } => {
                client_id.is_empty() && client_secret.is_empty()
            }
        };
        if missing {
            return Err(ExchangeError::MissingCredentials {
                auth_type: match &self.auth {
                    ExchangeAuth::Basic { .. } => "basic".into(),
                    ExchangeAuth::OAuth2 { .. } => "oauth2_client_credentials".into(),
                },
            });
        }
        Ok(())
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

/// Minimal `application/x-www-form-urlencoded` value encoder. Escapes
/// the characters that would otherwise break the form pair; Connected
/// App client ids/secrets are alphanumeric+`-`/`~` in practice, but a
/// secret could contain reserved characters.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Parse an OAuth2 token endpoint response body and return the
/// `access_token`.
pub fn parse_access_token(body: &[u8]) -> Result<String, ExchangeError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| ExchangeError::BadPayload(format!("token json decode: {e}")))?;
    v.get("access_token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .ok_or_else(|| {
            ExchangeError::MissingToken(String::from_utf8_lossy(body).to_string())
        })
}

/// Classifiers (in `packaging == "json"` files) that are preferred as
/// the canonical MCP descriptor, in no particular order.
const PREFERRED_DESCRIPTOR_CLASSIFIERS: &[&str] =
    &["mcp-metadata", "custom", "fat-mcp-metadata"];

/// Parse an Exchange asset metadata response and return the
/// pre-signed `externalLink` of the descriptor file to fetch.
///
/// The metadata body carries a `files` array; each entry has
/// `classifier`, `packaging`, and `externalLink`. The chosen file is the
/// FIRST entry with `packaging == "json"` AND a `classifier` in
/// [`PREFERRED_DESCRIPTOR_CLASSIFIERS`], falling back to the FIRST entry
/// with `packaging == "json"`. Returns `BadPayload` when no JSON file is
/// present or it lacks a usable `externalLink`.
pub fn parse_metadata_for_descriptor_link(body: &[u8]) -> Result<String, ExchangeError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| ExchangeError::BadPayload(format!("metadata json decode: {e}")))?;

    let files = v
        .get("files")
        .and_then(|f| f.as_array())
        .ok_or_else(|| ExchangeError::BadPayload("asset metadata missing files[] array".into()))?;

    let is_json = |f: &serde_json::Value| {
        f.get("packaging").and_then(|p| p.as_str()) == Some("json")
    };

    let chosen = files
        .iter()
        .find(|f| {
            is_json(f)
                && f.get("classifier")
                    .and_then(|c| c.as_str())
                    .map(|c| PREFERRED_DESCRIPTOR_CLASSIFIERS.contains(&c))
                    .unwrap_or(false)
        })
        .or_else(|| files.iter().find(|f| is_json(f)))
        .ok_or_else(|| {
            ExchangeError::BadPayload("no json descriptor file in asset metadata".into())
        })?;

    chosen
        .get("externalLink")
        .and_then(|l| l.as_str())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .ok_or_else(|| {
            ExchangeError::BadPayload(
                "chosen descriptor file has no externalLink".into(),
            )
        })
}

/// Parse the Exchange asset descriptor payload into a `PinSet`.
///
/// Several shapes are accepted so the pin works against either the live
/// Exchange asset file or a hand-authored fixture:
///
/// - `{ "version": "...", "tools": [<descriptor>...] }`
/// - `{ "assetVersion": "...", "tools": [<descriptor>...] }`
/// - `{ "payload": { "tools": [<descriptor>...] } }`
/// - `{ "mcp": { "tools": [<descriptor>...] } }`
/// - `{ "tools": [<descriptor>...] }`
///
/// `fallback_version` (the configured `exchange.version`) is used when
/// the body carries no version field.
pub fn parse_descriptor(
    body: &[u8],
    now_secs: u64,
    fallback_version: &str,
) -> Result<PinSet, ExchangeError> {
    let v: serde_json::Value = serde_json::from_slice(body)
        .map_err(|e| ExchangeError::BadPayload(format!("json decode: {e}")))?;

    let asset_version = v
        .get("version")
        .and_then(|s| s.as_str())
        .or_else(|| v.get("assetVersion").and_then(|s| s.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_version)
        .to_string();

    let tools = v
        .get("tools")
        .and_then(|t| t.as_array())
        .or_else(|| v.get("payload").and_then(|p| p.get("tools")).and_then(|t| t.as_array()))
        .or_else(|| v.get("mcp").and_then(|m| m.get("tools")).and_then(|t| t.as_array()))
        .ok_or_else(|| ExchangeError::BadPayload("missing tools[] array".into()))?
        .clone();

    Ok(PinSet::from_descriptors(&asset_version, now_secs, tools))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ref() -> ExchangeRef {
        ExchangeRef {
            base_url: "https://anypoint.mulesoft.com".into(),
            org_id: "o".into(),
            group_id: "g".into(),
            asset_id: "a".into(),
            version: "1.0.0".into(),
            path_prefix: String::new(),
            file_path_prefix: String::new(),
        }
    }

    #[test]
    fn descriptor_url_path() {
        let r = make_ref();
        assert_eq!(
            r.descriptor_url(),
            "https://anypoint.mulesoft.com/exchange/api/v2/assets/g/a/1.0.0/mcp.json"
        );
    }

    #[test]
    fn token_url_path() {
        let r = make_ref();
        assert_eq!(
            r.token_url(),
            "https://anypoint.mulesoft.com/accounts/api/v2/oauth2/token"
        );
    }

    #[test]
    fn path_prefix_is_prepended() {
        let mut r = make_ref();
        r.path_prefix = "/exchange-pin".into();
        assert_eq!(
            r.descriptor_path(),
            "/exchange-pin/exchange/api/v2/assets/g/a/1.0.0/mcp.json"
        );
        assert_eq!(r.token_path(), "/exchange-pin/accounts/api/v2/oauth2/token");
    }

    #[test]
    fn metadata_path_has_no_mcp_json_suffix() {
        let r = make_ref();
        assert_eq!(
            r.metadata_path(),
            "/exchange/api/v2/assets/g/a/1.0.0"
        );
        let mut r = make_ref();
        r.path_prefix = "/exchange-pin".into();
        assert_eq!(
            r.metadata_path(),
            "/exchange-pin/exchange/api/v2/assets/g/a/1.0.0"
        );
    }

    #[test]
    fn parse_metadata_picks_json_custom_file() {
        let body = br#"{
            "files": [
                {"classifier":"mule-plugin","packaging":"jar","externalLink":"https://s3/x.jar?sig=1"},
                {"classifier":"custom","packaging":"json","externalLink":"https://exchange2-asset-manager-kprod.s3.amazonaws.com/key.json?X-Amz-Signature=abc&X-Amz-SignedHeaders=host"},
                {"classifier":"other","packaging":"json","externalLink":"https://s3/y.json?sig=2"}
            ]
        }"#;
        let link = parse_metadata_for_descriptor_link(body).unwrap();
        assert_eq!(
            link,
            "https://exchange2-asset-manager-kprod.s3.amazonaws.com/key.json?X-Amz-Signature=abc&X-Amz-SignedHeaders=host"
        );
    }

    #[test]
    fn parse_metadata_falls_back_to_first_json() {
        let body = br#"{
            "files": [
                {"classifier":"mule-plugin","packaging":"jar","externalLink":"https://s3/x.jar"},
                {"classifier":"docs","packaging":"json","externalLink":"https://s3/first.json?sig=1"},
                {"classifier":"more","packaging":"json","externalLink":"https://s3/second.json?sig=2"}
            ]
        }"#;
        let link = parse_metadata_for_descriptor_link(body).unwrap();
        assert_eq!(link, "https://s3/first.json?sig=1");
    }

    #[test]
    fn parse_metadata_rejects_no_json_file() {
        let body = br#"{"files":[{"classifier":"mule-plugin","packaging":"jar","externalLink":"https://s3/x.jar"}]}"#;
        assert!(matches!(
            parse_metadata_for_descriptor_link(body),
            Err(ExchangeError::BadPayload(_))
        ));
    }

    #[test]
    fn file_loopback_path_strips_host_and_prepends_prefix() {
        let mut r = make_ref();
        r.file_path_prefix = "/exchange-s3".into();
        let link = "https://exchange2-asset-manager-kprod.s3.amazonaws.com/some/key.json?X-Amz-Signature=abc&X-Amz-SignedHeaders=host";
        assert_eq!(
            r.file_loopback_path(link).unwrap(),
            "/exchange-s3/some/key.json?X-Amz-Signature=abc&X-Amz-SignedHeaders=host"
        );
    }

    #[test]
    fn file_loopback_path_rejects_link_without_path() {
        let r = make_ref();
        assert!(matches!(
            r.file_loopback_path("https://host-only.example.com"),
            Err(ExchangeError::BadPayload(_))
        ));
    }

    #[test]
    fn basic_header_is_base64() {
        let c = ExchangeClient::new(
            make_ref(),
            ExchangeAuth::Basic { username: "u".into(), password: "p".into() },
        );
        assert_eq!(c.build_auth_header(), "Basic dTpw");
    }

    #[test]
    fn missing_credentials_detected() {
        let c = ExchangeClient::new(
            make_ref(),
            ExchangeAuth::OAuth2 { client_id: String::new(), client_secret: String::new() },
        );
        assert!(matches!(
            c.ensure_credentials(),
            Err(ExchangeError::MissingCredentials { .. })
        ));
    }

    #[test]
    fn parse_access_token_extracts_token() {
        let body = br#"{"access_token":"abc123","token_type":"bearer","expires_in":3600}"#;
        assert_eq!(parse_access_token(body).unwrap(), "abc123");
    }

    #[test]
    fn parse_access_token_rejects_empty() {
        let body = br#"{"token_type":"bearer"}"#;
        assert!(matches!(
            parse_access_token(body),
            Err(ExchangeError::MissingToken(_))
        ));
    }

    #[test]
    fn parse_descriptor_reads_version_and_tools() {
        let body = br#"{"version":"2.4.0","tools":[{"name":"get_user","description":"d"}]}"#;
        let pin = parse_descriptor(body, 42, "1.0.0").unwrap();
        assert_eq!(pin.asset_version, "2.4.0");
        assert_eq!(pin.fetched_at_epoch_secs, 42);
        assert!(pin.tools.contains_key("get_user"));
    }

    #[test]
    fn parse_descriptor_falls_back_to_configured_version() {
        let body = br#"{"tools":[{"name":"t","description":"d"}]}"#;
        let pin = parse_descriptor(body, 0, "9.9.9").unwrap();
        assert_eq!(pin.asset_version, "9.9.9");
    }

    #[test]
    fn parse_descriptor_accepts_payload_wrapper() {
        let body = br#"{"payload":{"tools":[{"name":"wrapped","description":"d"}]}}"#;
        let pin = parse_descriptor(body, 0, "1.0.0").unwrap();
        assert!(pin.tools.contains_key("wrapped"));
    }

    #[test]
    fn parse_descriptor_rejects_missing_tools() {
        let body = br#"{"version":"1.0.0"}"#;
        assert!(matches!(
            parse_descriptor(body, 0, "1.0.0"),
            Err(ExchangeError::BadPayload(_))
        ));
    }

    #[test]
    fn parse_descriptor_rejects_bad_json() {
        assert!(matches!(
            parse_descriptor(b"not json", 0, "1.0.0"),
            Err(ExchangeError::BadPayload(_))
        ));
    }
}
