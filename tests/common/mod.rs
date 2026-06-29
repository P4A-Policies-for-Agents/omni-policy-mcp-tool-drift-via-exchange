// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Helpers shared across integration tests.

#![allow(dead_code)]

use pdk_unit::{Backend, UnitHttpMessage, UnitHttpRequest, UnitHttpResponse};

pub struct RouterBackend {
    inner: Box<dyn Fn(UnitHttpRequest) -> UnitHttpResponse>,
}

impl RouterBackend {
    pub fn new<F: Fn(UnitHttpRequest) -> UnitHttpResponse + 'static>(f: F) -> Self {
        Self { inner: Box::new(f) }
    }
}

impl Backend for RouterBackend {
    fn call(&self, req: UnitHttpRequest) -> UnitHttpResponse {
        (self.inner)(req)
    }
}

pub fn json(status: u32, body: &str) -> UnitHttpResponse {
    UnitHttpResponse::new(status)
        .with_header("content-type", "application/json")
        .with_body(body.as_bytes().to_vec())
}

pub fn tool(name: &str, description: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object"},
    })
}

pub fn tools_list_body(id: u64, tools: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"tools": tools},
    })
    .to_string()
}

pub fn policy_config(mode: &str) -> String {
    serde_json::json!({
        "exchange": {
            "orgId": "demo-org",
            "groupId": "demo-group",
            "assetId": "demo-mcp-asset",
            "version": "1.0.0",
            "baseUrl": "https://anypoint.mulesoft.com",
            "authType": "oauth2_client_credentials",
            "credSecretRef": "exchange-creds",
            "refreshIntervalSec": 300
        },
        "enforce": {
            "exactMatch": true,
            "allowAddedTools": false,
            "allowRemovedTools": true
        },
        "mode": mode,
        "failOpen": {"onPinUnavailable": true}
    })
    .to_string()
}
