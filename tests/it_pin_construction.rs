// Copyright 2026 Salesforce, Inc. All rights reserved.

//! PinSet — construction, name keying, hash stability.

mod common;

use omni_policy_mcp_tool_drift_via_exchange::pin::{canonical_hash, PinSet};

#[test]
fn pinset_keys_by_name() {
    let descs = vec![
        common::tool("get_user", "lookup"),
        common::tool("list_orders", "list"),
    ];
    let pin = PinSet::from_descriptors("v1", 0, descs);
    assert!(pin.tools.contains_key("get_user"));
    assert!(pin.tools.contains_key("list_orders"));
    assert_eq!(pin.tools.len(), 2);
}

#[test]
fn pinset_drops_descriptors_without_name() {
    let descs = vec![
        common::tool("get_user", "lookup"),
        serde_json::json!({"description": "no-name"}),
    ];
    let pin = PinSet::from_descriptors("v1", 0, descs);
    assert_eq!(pin.tools.len(), 1);
}

#[test]
fn canonical_hash_stable_across_key_order() {
    let a = serde_json::json!({"name": "x", "description": "y"});
    let b = serde_json::json!({"description": "y", "name": "x"});
    assert_eq!(canonical_hash(&a), canonical_hash(&b));
}

#[test]
fn canonical_hash_changes_on_description() {
    let a = common::tool("t", "v1");
    let b = common::tool("t", "v2");
    assert_ne!(canonical_hash(&a), canonical_hash(&b));
}
