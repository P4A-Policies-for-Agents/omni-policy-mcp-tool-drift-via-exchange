//! Minimal JSON-RPC 2.0 envelope. MCP traffic is JSON-RPC over HTTP;
//! the policy inspects `tools/list` responses.

use serde::{Deserialize, Serialize};

pub const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub fn extract_tools_array(resp: &JsonRpcResponse) -> Option<&Vec<serde_json::Value>> {
    resp.result
        .as_ref()
        .and_then(|r| r.get("tools"))
        .and_then(|v| v.as_array())
}
