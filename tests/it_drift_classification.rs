// Copyright 2026 Salesforce, Inc. All rights reserved.

//! classify() — unchanged / description / schema / annotation / name.

mod common;

use omni_policy_mcp_tool_drift_via_exchange::pin::{classify, DriftField, PinnedTool};

#[test]
fn unchanged_descriptor_is_none() {
    let pin = PinnedTool::from_descriptor(common::tool("get_user", "lookup")).unwrap();
    assert_eq!(classify(&pin, &common::tool("get_user", "lookup")), None);
}

#[test]
fn description_drift_classified() {
    let pin = PinnedTool::from_descriptor(common::tool("get_user", "safe")).unwrap();
    let runtime = common::tool("get_user", "DRIFTED");
    assert_eq!(classify(&pin, &runtime), Some(DriftField::Description));
}

#[test]
fn schema_drift_classified() {
    let pinned = serde_json::json!({
        "name": "get_user",
        "description": "lookup",
        "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}},
    });
    let runtime = serde_json::json!({
        "name": "get_user",
        "description": "lookup",
        "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}}},
    });
    let pin = PinnedTool::from_descriptor(pinned).unwrap();
    assert_eq!(classify(&pin, &runtime), Some(DriftField::InputSchema));
}

#[test]
fn annotation_drift_classified() {
    let pinned = serde_json::json!({
        "name": "delete_user",
        "description": "remove",
        "annotations": {"destructiveHint": false},
    });
    let runtime = serde_json::json!({
        "name": "delete_user",
        "description": "remove",
        "annotations": {"destructiveHint": true},
    });
    let pin = PinnedTool::from_descriptor(pinned).unwrap();
    assert_eq!(classify(&pin, &runtime), Some(DriftField::Annotations));
}
