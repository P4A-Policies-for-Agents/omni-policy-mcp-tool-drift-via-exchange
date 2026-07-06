//! Pinned descriptor set sourced from the Anypoint Exchange asset and
//! cached locally. Each pinned descriptor carries a canonical hash so
//! a runtime descriptor can be compared in O(1).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Canonical hash over the four fields that determine a tool's
/// behavior contract from the LLM's perspective. Field order is fixed;
/// absent fields are omitted (not null).
pub fn canonical_hash(tool: &serde_json::Value) -> String {
    let mut canon = serde_json::Map::new();
    for key in ["name", "description", "inputSchema", "outputSchema", "annotations"] {
        if let Some(v) = tool.get(key) {
            canon.insert(key.to_string(), canonical_field(key, v));
        }
    }
    let bytes = serde_json::to_vec(&serde_json::Value::Object(canon))
        .expect("canonical map serializes");
    let mut h = Sha256::new();
    h.update(&bytes);
    hex_encode(&h.finalize())
}

fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = m.iter().collect();
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k.clone(), canonicalize(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// JSON-Schema keys that carry no behavioral contract. Different schema
/// serializers add or drop these freely (e.g. an MCP server that
/// regenerates strict draft-07 schemas at runtime vs. the minimal copy
/// stored at authoring time), so they are removed before hashing to stop
/// cosmetic serialization differences from reading as tool drift.
const SCHEMA_META_KEYS: &[&str] = &["$schema", "$id", "$comment"];

/// Normalize a JSON Schema value for hashing:
/// - drop metadata-only keys (`$schema`, `$id`, `$comment`),
/// - fold the strict-default `additionalProperties: false` to "absent"
///   so a serializer that emits it hashes the same as one that omits it.
///
/// Only provably cosmetic differences are folded. A real contract change
/// — `type`/`properties`/`required`/`enum`/nested `description`, or
/// loosening `additionalProperties` to `true`/a subschema — still changes
/// the hash and is reported as drift.
fn normalize_schema(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if SCHEMA_META_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if k == "additionalProperties" && val.as_bool() == Some(false) {
                    continue;
                }
                out.insert(k.clone(), normalize_schema(val));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(normalize_schema).collect())
        }
        other => other.clone(),
    }
}

/// Canonicalize one top-level descriptor field. Schema-bearing fields are
/// first run through `normalize_schema` so semantically-equal schemas
/// hash identically regardless of serializer decoration.
fn canonical_field(key: &str, v: &serde_json::Value) -> serde_json::Value {
    match key {
        "inputSchema" | "outputSchema" => canonicalize(&normalize_schema(v)),
        _ => canonicalize(v),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedTool {
    pub name: String,
    pub hash: String,
    pub descriptor: serde_json::Value,
}

impl PinnedTool {
    pub fn from_descriptor(d: serde_json::Value) -> Option<Self> {
        let name = d.get("name")?.as_str()?.to_string();
        let hash = canonical_hash(&d);
        Some(Self { name, hash, descriptor: d })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PinSet {
    pub asset_version: String,
    pub fetched_at_epoch_secs: u64,
    pub tools: BTreeMap<String, PinnedTool>,
}

impl PinSet {
    pub fn from_descriptors(asset_version: &str, now_secs: u64, descs: Vec<serde_json::Value>) -> Self {
        let mut tools = BTreeMap::new();
        for d in descs {
            if let Some(p) = PinnedTool::from_descriptor(d) {
                tools.insert(p.name.clone(), p);
            }
        }
        Self {
            asset_version: asset_version.to_string(),
            fetched_at_epoch_secs: now_secs,
            tools,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftField {
    Description,
    InputSchema,
    OutputSchema,
    Annotations,
    NameOrUnknown,
}

impl DriftField {
    pub fn label(self) -> &'static str {
        match self {
            DriftField::Description => "description_changed",
            DriftField::InputSchema => "input_schema_changed",
            DriftField::OutputSchema => "output_schema_changed",
            DriftField::Annotations => "annotation_changed",
            DriftField::NameOrUnknown => "descriptor_changed",
        }
    }
}

/// Compare a runtime descriptor to a pinned one; report which field
/// differs. None means byte-identical after canonicalisation.
pub fn classify(pin: &PinnedTool, runtime: &serde_json::Value) -> Option<DriftField> {
    if canonical_hash(runtime) == pin.hash {
        return None;
    }
    for (key, field) in [
        ("description", DriftField::Description),
        ("inputSchema", DriftField::InputSchema),
        ("outputSchema", DriftField::OutputSchema),
        ("annotations", DriftField::Annotations),
    ] {
        let a = pin.descriptor.get(key).map(|v| canonical_field(key, v));
        let b = runtime.get(key).map(|v| canonical_field(key, v));
        if a != b {
            return Some(field);
        }
    }
    Some(DriftField::NameOrUnknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a runtime serializer that adds `$schema` +
    /// `additionalProperties: false` must NOT read as drift against a pin
    /// stored without those decorations.
    #[test]
    fn schema_serializer_decorations_do_not_drift() {
        let pinned = serde_json::json!({
            "name": "t", "description": "x",
            "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}
        });
        let runtime = serde_json::json!({
            "name": "t", "description": "x",
            "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"],
                "additionalProperties": false, "$schema": "http://json-schema.org/draft-07/schema#"}
        });
        assert_eq!(canonical_hash(&pinned), canonical_hash(&runtime));
        let pin = PinnedTool::from_descriptor(pinned).unwrap();
        assert_eq!(classify(&pin, &runtime), None);
    }

    fn tool(name: &str, desc: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "description": desc,
            "inputSchema": {"type": "object"},
        })
    }

    #[test]
    fn identical_tools_share_hash() {
        let a = tool("get_user", "lookup");
        let b = tool("get_user", "lookup");
        assert_eq!(canonical_hash(&a), canonical_hash(&b));
    }

    #[test]
    fn description_drift_classified() {
        let pin = PinnedTool::from_descriptor(tool("t", "safe")).unwrap();
        let runtime = tool("t", "DRIFTED");
        assert_eq!(classify(&pin, &runtime), Some(DriftField::Description));
    }
}
