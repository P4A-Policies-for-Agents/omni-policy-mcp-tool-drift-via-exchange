// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Smoke check: extract_tools_array surfaces the runtime tools array
//! out of a JSON-RPC `tools/list` response.

mod common;

use omni_policy_mcp_tool_drift_via_exchange::jsonrpc::{extract_tools_array, JsonRpcResponse};

#[test]
fn extracts_tools_array() {
    let body = common::tools_list_body(1, vec![common::tool("a", "x"), common::tool("b", "y")]);
    let resp: JsonRpcResponse = serde_json::from_str(&body).unwrap();
    let tools = extract_tools_array(&resp).expect("tools array present");
    assert_eq!(tools.len(), 2);
}

#[test]
fn returns_none_for_error_response() {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {"code": -32601, "message": "method not found"},
    })
    .to_string();
    let resp: JsonRpcResponse = serde_json::from_str(&body).unwrap();
    assert!(extract_tools_array(&resp).is_none());
}

#[test]
fn returns_none_for_non_tools_result() {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"resources": []},
    })
    .to_string();
    let resp: JsonRpcResponse = serde_json::from_str(&body).unwrap();
    assert!(extract_tools_array(&resp).is_none());
}
