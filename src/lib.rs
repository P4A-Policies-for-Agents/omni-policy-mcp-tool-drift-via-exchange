//! MCP Tool Drift Detection (Exchange) — policy entrypoint.

pub mod config;
pub mod evidence;
pub mod exchange;
pub mod generated;
pub mod jsonrpc;
pub mod pin;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use pdk::cache::CacheBuilder;
use pdk::hl::*;
use pdk::logger;

use crate::config::{Mode, PolicyConfig};
use crate::evidence::{Decision, DetectionClass, Event, Severity};
use crate::generated::config::Config;
use crate::pin::{classify, PinSet};

const CONTENT_LENGTH_HEADER: &str = "content-length";

#[derive(Clone)]
struct PolicyState {
    cfg: Rc<PolicyConfig>,
    pin: Rc<RefCell<Option<PinSet>>>,
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

async fn response_filter(
    resp_state: ResponseState,
    state: PolicyState,
) {
    let headers_state = resp_state.into_headers_state().await;
    let ct = headers_state
        .handler()
        .headers()
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v)
        .unwrap_or_default();

    // SSE: log and pass through
    if ct.contains("text/event-stream") {
        logger::debug!("mcp-drift-exchange: SSE response detected, pass-through");
        return;
    }

    if !ct.contains("application/json") {
        return;
    }

    // Strip content-length on the headers handler BEFORE moving to body state.
    headers_state.handler().remove_header(CONTENT_LENGTH_HEADER);

    let body_state = headers_state.into_body_state().await;
    let body = body_state.handler().body().to_vec();
    let resp: jsonrpc::JsonRpcResponse = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return,
    };

    // Notifications (id missing or null): always pass through
    if resp.id.is_none() || matches!(resp.id, Some(serde_json::Value::Null)) {
        return;
    }

    let Some(tools) = jsonrpc::extract_tools_array(&resp) else {
        return;
    };

    let pin_borrow = state.pin.borrow().clone();
    let Some(pin) = pin_borrow else {
        if !state.cfg.fail_open_on_pin_unavailable {
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
            }
            .emit();
        }
        return;
    };

    let mut kept: Vec<serde_json::Value> = Vec::with_capacity(tools.len());
    for tool in tools.iter() {
        let Some(name) = tool.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let mut keep = true;
        if let Some(pinned) = pin.tools.get(name) {
            if state.cfg.enforce.exact_match {
                if let Some(field) = classify(pinned, tool) {
                    let runtime_hash = pin::canonical_hash(tool);
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
                    }
                    .emit();
                    keep = !matches!(state.cfg.mode, Mode::Enforce);
                }
            }
        } else {
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
            }
            .emit();
            if !state.cfg.enforce.allow_added_tools && matches!(state.cfg.mode, Mode::Enforce) {
                keep = false;
            }
        }
        if keep {
            kept.push(tool.clone());
        }
    }

    let runtime_names: std::collections::HashSet<&str> = kept
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .collect();
    for pinned_name in pin.tools.keys() {
        if !runtime_names.contains(pinned_name.as_str()) && state.cfg.enforce.allow_removed_tools {
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
            }
            .emit();
        }
    }

    if matches!(state.cfg.mode, Mode::Enforce) {
        let rewritten = rewrite_tools_list(&resp, kept);
        // content-length already stripped on headers handler above.
        let _ = body_state.handler().set_body(&rewritten);
    }
}

fn rewrite_tools_list(resp: &jsonrpc::JsonRpcResponse, kept: Vec<serde_json::Value>) -> Vec<u8> {
    let mut new_resp = resp.clone();
    let mut result = new_resp.result.unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = result.as_object_mut() {
        map.insert("tools".into(), serde_json::Value::Array(kept));
    }
    new_resp.result = Some(result);
    serde_json::to_vec(&new_resp).expect("response serializes")
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
        "mcp-drift-exchange: loaded asset={}/{}/{} base={} mode={:?}",
        cfg.exchange.group_id,
        cfg.exchange.asset_id,
        cfg.exchange.version,
        cfg.exchange.base_url,
        cfg.mode,
    );

    let state = PolicyState {
        cfg: Rc::new(cfg),
        pin: Rc::new(RefCell::new(None)),
    };

    // Pin refresh via HttpClient is injected per-filter once wired up;
    // until then the response-path emits `pin_unavailable` evidence
    // and obeys `failOpen.onPinUnavailable`.

    let filter = on_response(move |resp: ResponseState| {
        let s = state.clone();
        async move { response_filter(resp, s).await }
    });
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
