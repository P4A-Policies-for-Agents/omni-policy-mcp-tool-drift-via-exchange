// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Exchange URL construction — descriptor & token endpoints.

mod common;

use omni_policy_mcp_tool_drift_via_exchange::exchange::ExchangeRef;

fn make_ref(base: &str) -> ExchangeRef {
    ExchangeRef {
        base_url: base.into(),
        org_id: "o".into(),
        group_id: "g".into(),
        asset_id: "a".into(),
        version: "1.0.0".into(),
        path_prefix: String::new(),
    }
}

#[test]
fn descriptor_url_uses_v2_path() {
    let r = make_ref("https://anypoint.mulesoft.com");
    assert_eq!(
        r.descriptor_url(),
        "https://anypoint.mulesoft.com/exchange/api/v2/assets/g/a/1.0.0/mcp.json"
    );
}

#[test]
fn token_url_uses_accounts_path() {
    let r = make_ref("https://anypoint.mulesoft.com");
    assert_eq!(
        r.token_url(),
        "https://anypoint.mulesoft.com/accounts/api/v2/oauth2/token"
    );
}

#[test]
fn trailing_slash_is_trimmed() {
    let r = make_ref("https://eu1.anypoint.mulesoft.com/");
    assert!(r.descriptor_url().starts_with("https://eu1.anypoint.mulesoft.com/exchange"));
}
