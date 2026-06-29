// Copyright 2026 Salesforce, Inc. All rights reserved.

//! policy_config() permutations deserialize through the generated
//! config and into PolicyConfig.

mod common;

use omni_policy_mcp_tool_drift_via_exchange::config::{AuthType, Mode, PolicyConfig};
use omni_policy_mcp_tool_drift_via_exchange::generated::config::Config;

fn load(mode: &str) -> PolicyConfig {
    let raw: Config = serde_json::from_str(&common::policy_config(mode)).unwrap();
    PolicyConfig::from_config(&raw).unwrap()
}

#[test]
fn enforce_loads() {
    let c = load("enforce");
    assert_eq!(c.mode, Mode::Enforce);
    assert_eq!(c.exchange.auth_type, AuthType::OAuth2ClientCredentials);
    assert!(c.enforce.exact_match);
    assert!(!c.enforce.allow_added_tools);
    assert!(c.enforce.allow_removed_tools);
}

#[test]
fn observe_loads() {
    let c = load("observe");
    assert_eq!(c.mode, Mode::Observe);
}

#[test]
fn warn_loads() {
    let c = load("warn");
    assert_eq!(c.mode, Mode::Warn);
}
