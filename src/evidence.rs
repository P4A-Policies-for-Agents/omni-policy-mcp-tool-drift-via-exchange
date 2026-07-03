//! Structured evidence events emitted as JSON-encoded log lines so
//! Anypoint Analytics indexes them inside the customer tenant.

use pdk::logger;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionClass {
    DescriptorDrift,
    UnpinnedTool,
    RemovedTool,
    VersionChanged,
    PinStale,
    PinUnavailable,
}

impl DetectionClass {
    /// Stable snake_case label used as the debounce map key. Matches the
    /// serialized JSON representation so operators searching evidence
    /// logs and audit tables can grep for one name across both surfaces.
    pub fn debounce_label(self) -> &'static str {
        match self {
            DetectionClass::DescriptorDrift => "descriptor_drift",
            DetectionClass::UnpinnedTool => "unpinned_tool",
            DetectionClass::RemovedTool => "removed_tool",
            DetectionClass::VersionChanged => "version_changed",
            DetectionClass::PinStale => "pin_stale",
            DetectionClass::PinUnavailable => "pin_unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allowed,
    Blocked,
    Stripped,
    Annotated,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event<'a> {
    pub class: DetectionClass,
    pub severity: Severity,
    pub decision: Decision,
    pub asset_id: &'a str,
    pub asset_version: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub pin_hash: Option<&'a str>,
    pub runtime_hash: Option<&'a str>,
    pub field: Option<&'a str>,
    pub note: Option<&'a str>,
}

impl<'a> Event<'a> {
    pub fn emit(&self) {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        match self.severity {
            Severity::Critical => logger::error!("mcp-drift-exchange-evt {}", json),
            Severity::Warning => logger::warn!("mcp-drift-exchange-evt {}", json),
            Severity::Info => logger::info!("mcp-drift-exchange-evt {}", json),
        }
    }
}
