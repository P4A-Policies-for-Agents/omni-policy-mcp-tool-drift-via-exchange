//! MCP Tool Drift Detection (Exchange) — policy entrypoint.

pub mod config;
pub mod debounce;
pub mod evidence;
pub mod exchange;
pub mod generated;
pub mod jsonrpc;
pub mod pin;
pub mod sse;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use pdk::cache::CacheBuilder;
use pdk::hl::*;
use pdk::logger;

use crate::config::{AuthType, Mode, PolicyConfig};
use crate::debounce::{now_epoch_secs, Debouncer};
use crate::evidence::{Decision, DetectionClass, Event, Severity};
use crate::exchange::{ExchangeAuth, ExchangeClient, ExchangeRef as ExchangeAssetRef};
use crate::generated::config::Config;
use crate::pin::{classify, PinSet};

const CONTENT_LENGTH_HEADER: &str = "content-length";

#[derive(Clone)]
struct PolicyState {
    cfg: Rc<PolicyConfig>,
    pin: Rc<RefCell<Option<PinSet>>>,
    debouncer: Rc<RefCell<Debouncer>>,
}

/// Discover the upstream cluster Envoy selected for the current request
/// from the stream properties. This is the cluster the API instance
/// itself proxies to, which the gateway already routes with the correct
/// `Host`. It is generally populated once routing has picked an upstream
/// — reliably in the response phase, and often in the request phase too.
fn cluster_from_props(props: &StreamProperties) -> Option<String> {
    for path in [&["cluster_name"][..], &["xds", "cluster_name"][..]] {
        if let Some(bytes) = props.read_property(path) {
            if let Ok(s) = std::str::from_utf8(&bytes) {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Resolve the `Service` used for outbound Exchange calls.
///
/// A policy-registered `format: service` upstream is dispatched to a
/// synthetic Envoy cluster whose egress `:authority` is mangled by the
/// managed gateway's host-rewrite, so Exchange rejects the call. When a
/// loopback `exchangePathPrefix` is configured we dispatch to the
/// configured `baseUrl` (the gateway's own internal listener) verbatim
/// and skip upstream-cluster discovery; the prefixed path re-enters
/// through a passthrough route whose `auto_host_rewrite` restores the
/// correct `Host`. Otherwise we reuse the request's own upstream cluster
/// (discovered via the `cluster_name` stream property, or the
/// `x-envoy-decorator-operation` response header as a fallback). If the
/// cluster can't be discovered, `allow_config_fallback` permits the
/// legacy `format: service` path as a last resort.
fn resolve_outbound_service(
    state: &PolicyState,
    props: &StreamProperties,
    decorator: Option<&str>,
    allow_config_fallback: bool,
) -> Option<Service> {
    if !state.cfg.exchange.path_prefix.is_empty() {
        return state.cfg.exchange.service.clone();
    }
    let cluster = cluster_from_props(props).or_else(|| {
        decorator
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    if let Some(cluster) = cluster {
        if let Ok(uri) = state.cfg.exchange.base_url.parse::<Uri>() {
            return Some(Service::new(&cluster, uri));
        }
    }
    if allow_config_fallback {
        return state.cfg.exchange.service.clone();
    }
    None
}

/// Build the Exchange auth handle from the resolved credential secret.
///
/// The credential value is expected to be a colon-joined pair —
/// `clientId:clientSecret` for `oauth2_client_credentials`, or
/// `username:password` for `basic`. When no colon is present the whole
/// value is treated as the first component.
fn build_exchange_auth(cfg: &crate::config::ExchangeRef) -> ExchangeAuth {
    let (first, second) = cfg
        .cred_secret_ref
        .split_once(':')
        .unwrap_or((cfg.cred_secret_ref.as_str(), ""));
    match cfg.auth_type {
        AuthType::Basic => ExchangeAuth::Basic {
            username: first.to_string(),
            password: second.to_string(),
        },
        AuthType::OAuth2ClientCredentials => ExchangeAuth::OAuth2 {
            client_id: first.to_string(),
            client_secret: second.to_string(),
        },
    }
}

/// Lazy pin fetch. Runs on the first request/response with no pin
/// loaded, and again after the configured refresh interval has elapsed.
/// Outbound HTTPS from the request/response phases works under
/// connected-mode Flex Gateway; the same call from `configure()` never
/// connects. Last-known-good is preserved on failure.
async fn ensure_pin_loaded(state: &PolicyState, client: &HttpClient, service: &Service) {
    let now = now_epoch_secs();
    let should_refresh = {
        let borrow = state.pin.borrow();
        match borrow.as_ref() {
            None => true,
            Some(pin) => {
                let age = now.saturating_sub(pin.fetched_at_epoch_secs);
                age >= state.cfg.exchange.refresh_interval_secs.max(1) as u64
            }
        }
    };
    if !should_refresh {
        return;
    }

    let reference = ExchangeAssetRef {
        base_url: state.cfg.exchange.base_url.clone(),
        org_id: state.cfg.exchange.org_id.clone(),
        group_id: state.cfg.exchange.group_id.clone(),
        asset_id: state.cfg.exchange.asset_id.clone(),
        version: state.cfg.exchange.version.clone(),
        path_prefix: state.cfg.exchange.path_prefix.clone(),
        file_path_prefix: state.cfg.exchange.file_path_prefix.clone(),
    };
    let auth = build_exchange_auth(&state.cfg.exchange);
    let exchange = ExchangeClient::new(reference, auth);

    match exchange.fetch(client, service, now).await {
        Ok(pin) => {
            let asset_version = pin.asset_version.clone();
            let tool_count = pin.tools.len();
            let first_load = state.pin.borrow().is_none();
            state.pin.replace(Some(pin));
            logger::info!(
                "mcp-drift-exchange: pin loaded (first_load={} asset_version={} tools={})",
                first_load,
                asset_version,
                tool_count
            );
        }
        Err(e) => {
            logger::warn!("mcp-drift-exchange: pin fetch failed: {e}");
        }
    }
}

fn emit_debounced(event: Event<'_>, state: &PolicyState, now_secs: u64) {
    let tool_key = event.tool_name.unwrap_or("<policy>");
    let class_label = event.class.debounce_label();
    if state
        .debouncer
        .borrow_mut()
        .should_emit(tool_key, class_label, now_secs)
    {
        event.emit();
    }
}

fn decision_for(mode: Mode, would_block: bool) -> Decision {
    match (mode, would_block) {
        (Mode::Enforce, true) => Decision::Stripped,
        (Mode::Enforce, false) => Decision::Allowed,
        (Mode::Observe, _) => Decision::Allowed,
        (Mode::Warn, true) => Decision::Annotated,
        (Mode::Warn, false) => Decision::Allowed,
    }
}

/// Request-phase handler. We do not inspect the request body; we run
/// here only to warm the pin cache with an outbound HTTPS call when the
/// upstream cluster is already known. Outbound HTTPS is safe in the
/// request-headers phase (before the body-state transition). We do NOT
/// fall back to the config `format: service` cluster here because its
/// egress `Host` is mangled by the managed gateway; that fallback is
/// only allowed (via loopback or last-resort) in the response phase.
async fn request_filter(
    _request: RequestHeadersState,
    state: PolicyState,
    client: HttpClient,
    props: StreamProperties,
) -> Flow<()> {
    if let Some(service) = resolve_outbound_service(&state, &props, None, false) {
        ensure_pin_loaded(&state, &client, &service).await;
    }
    Flow::Continue(())
}

async fn response_filter(
    headers_state: ResponseHeadersState,
    state: PolicyState,
    client: HttpClient,
    _data: RequestData<()>,
    props: StreamProperties,
) {
    let headers = headers_state.handler().headers();
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let is_sse = ct.contains("text/event-stream");
    let is_json = ct.contains("application/json");
    if !is_sse && !is_json {
        return;
    }

    // Second-chance pin load. The response phase is a safe outbound
    // context AND the phase where the upstream cluster is reliably known,
    // so resolve the outbound service to the request's own upstream
    // cluster (with `x-envoy-decorator-operation` as a fallback source).
    let decorator = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-envoy-decorator-operation"))
        .map(|(_, v)| v.clone());
    if let Some(service) = resolve_outbound_service(&state, &props, decorator.as_deref(), true) {
        ensure_pin_loaded(&state, &client, &service).await;
    }

    // Strip content-length on the headers handler BEFORE moving to body state.
    headers_state.handler().remove_header(CONTENT_LENGTH_HEADER);

    let body_state = headers_state.into_body_state().await;
    let body = body_state.handler().body().to_vec();

    let rewritten: Option<Vec<u8>> = if is_sse {
        enforce_sse(&body, &state)
    } else {
        enforce_json(&body, &state)
    };

    if let Some(new_body) = rewritten {
        let _ = body_state.handler().set_body(&new_body);
    }
}

fn enforce_sse(body: &[u8], state: &PolicyState) -> Option<Vec<u8>> {
    let mut events = sse::parse(body);
    let mut mutated = false;
    for ev in events.iter_mut() {
        let Some(data) = ev.data.as_deref() else {
            continue;
        };
        let Ok(resp) = serde_json::from_str::<jsonrpc::JsonRpcResponse>(data) else {
            continue;
        };
        if let Some(new_resp) = apply_policy(&resp, state) {
            let Ok(new_data) = serde_json::to_string(&new_resp) else {
                continue;
            };
            ev.data = Some(new_data);
            mutated = true;
        }
    }
    if !mutated {
        return None;
    }
    Some(sse::serialize(&events))
}

fn enforce_json(body: &[u8], state: &PolicyState) -> Option<Vec<u8>> {
    let resp: jsonrpc::JsonRpcResponse = serde_json::from_slice(body).ok()?;
    let new_resp = apply_policy(&resp, state)?;
    serde_json::to_vec(&new_resp).ok()
}

fn apply_policy(
    resp: &jsonrpc::JsonRpcResponse,
    state: &PolicyState,
) -> Option<jsonrpc::JsonRpcResponse> {
    if resp.id.is_none() || matches!(resp.id, Some(serde_json::Value::Null)) {
        return None;
    }

    let tools = jsonrpc::extract_tools_array(resp)?;

    let now_secs = now_epoch_secs();

    let pin_borrow = state.pin.borrow().clone();
    let Some(pin) = pin_borrow else {
        if !state.cfg.fail_open_on_pin_unavailable {
            emit_debounced(
                Event {
                    class: DetectionClass::PinUnavailable,
                    severity: Severity::Critical,
                    decision: Decision::Blocked,
                    asset_id: &state.cfg.exchange.asset_id,
                    asset_version: Some(&state.cfg.exchange.version),
                    tool_name: None,
                    pin_hash: None,
                    runtime_hash: None,
                    field: None,
                    note: Some("no pin loaded; failing closed"),
                },
                state,
                now_secs,
            );
        }
        return None;
    };

    let mut kept: Vec<serde_json::Value> = Vec::with_capacity(tools.len());
    let mut stripped_any = false;
    for tool in tools.iter() {
        let Some(name) = tool.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut keep = true;
        if let Some(pinned) = pin.tools.get(name) {
            if state.cfg.enforce.exact_match {
                if let Some(field) = classify(pinned, tool) {
                    let runtime_hash = pin::canonical_hash(tool);
                    emit_debounced(
                        Event {
                            class: DetectionClass::DescriptorDrift,
                            severity: Severity::Critical,
                            decision: decision_for(state.cfg.mode, true),
                            asset_id: &state.cfg.exchange.asset_id,
                            asset_version: Some(&pin.asset_version),
                            tool_name: Some(name),
                            pin_hash: Some(&pinned.hash),
                            runtime_hash: Some(&runtime_hash),
                            field: Some(field.label()),
                            note: None,
                        },
                        state,
                        now_secs,
                    );
                    keep = !matches!(state.cfg.mode, Mode::Enforce);
                }
            }
        } else {
            emit_debounced(
                Event {
                    class: DetectionClass::UnpinnedTool,
                    severity: Severity::Warning,
                    decision: decision_for(state.cfg.mode, !state.cfg.enforce.allow_added_tools),
                    asset_id: &state.cfg.exchange.asset_id,
                    asset_version: Some(&pin.asset_version),
                    tool_name: Some(name),
                    pin_hash: None,
                    runtime_hash: None,
                    field: None,
                    note: None,
                },
                state,
                now_secs,
            );
            if !state.cfg.enforce.allow_added_tools && matches!(state.cfg.mode, Mode::Enforce) {
                keep = false;
            }
        }
        if keep {
            kept.push(tool.clone());
        } else {
            stripped_any = true;
        }
    }

    let runtime_names: std::collections::HashSet<&str> = kept
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    for pinned_name in pin.tools.keys() {
        if !runtime_names.contains(pinned_name.as_str()) && state.cfg.enforce.allow_removed_tools {
            emit_debounced(
                Event {
                    class: DetectionClass::RemovedTool,
                    severity: Severity::Info,
                    decision: Decision::Allowed,
                    asset_id: &state.cfg.exchange.asset_id,
                    asset_version: Some(&pin.asset_version),
                    tool_name: Some(pinned_name),
                    pin_hash: None,
                    runtime_hash: None,
                    field: None,
                    note: None,
                },
                state,
                now_secs,
            );
        }
    }

    if matches!(state.cfg.mode, Mode::Enforce) && stripped_any {
        Some(rewrite_tools_list(resp, kept))
    } else {
        None
    }
}

fn rewrite_tools_list(
    resp: &jsonrpc::JsonRpcResponse,
    kept: Vec<serde_json::Value>,
) -> jsonrpc::JsonRpcResponse {
    let mut new_resp = resp.clone();
    let mut result = new_resp.result.unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = result.as_object_mut() {
        map.insert("tools".into(), serde_json::Value::Array(kept));
    }
    new_resp.result = Some(result);
    new_resp
}

#[entrypoint]
pub async fn configure(
    launcher: Launcher,
    Configuration(bytes): Configuration,
    _cache_builder: CacheBuilder,
) -> anyhow::Result<()> {
    let raw: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("invalid policy configuration: {e}"))?;
    let cfg = PolicyConfig::from_config(&raw)
        .map_err(|e| anyhow!("policy configuration rejected: {e}"))?;

    logger::info!(
        "mcp-drift-exchange: loaded asset={}/{}/{} base={} mode={:?} authType={:?} refresh={}s loopback={} service_bound={}",
        cfg.exchange.group_id,
        cfg.exchange.asset_id,
        cfg.exchange.version,
        cfg.exchange.base_url,
        cfg.mode,
        cfg.exchange.auth_type,
        cfg.exchange.refresh_interval_secs,
        !cfg.exchange.path_prefix.is_empty(),
        cfg.exchange.service.is_some(),
    );

    let state = PolicyState {
        cfg: Rc::new(cfg),
        pin: Rc::new(RefCell::new(None)),
        debouncer: Rc::new(RefCell::new(Debouncer::default())),
    };

    if state.cfg.exchange.service.is_none() {
        logger::warn!(
            "mcp-drift-exchange: baseUrl service unbound; response path will emit pin_unavailable evidence and obey failOpen.onPinUnavailable"
        );
    }

    let request_state = state.clone();
    let response_state = state;
    let filter = on_request(
        move |request: RequestHeadersState, client: HttpClient, props: StreamProperties| {
            let s = request_state.clone();
            async move { request_filter(request, s, client, props).await }
        },
    )
    .on_response(
        move |response: ResponseHeadersState,
              client: HttpClient,
              data: RequestData<()>,
              props: StreamProperties| {
            let s = response_state.clone();
            async move { response_filter(response, s, client, data, props).await }
        },
    );

    launcher.launch(filter).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_for_enforce_blocks() {
        assert!(matches!(decision_for(Mode::Enforce, true), Decision::Stripped));
        assert!(matches!(decision_for(Mode::Observe, true), Decision::Allowed));
        assert!(matches!(decision_for(Mode::Warn, true), Decision::Annotated));
    }
}
