use serde::Deserialize;
#[derive(Deserialize, Clone, Debug)]
pub struct EnforceConfig {
    #[serde(alias = "allowAddedTools")]
    pub allow_added_tools: Option<bool>,
    #[serde(alias = "allowRemovedTools")]
    pub allow_removed_tools: Option<bool>,
    #[serde(alias = "exactMatch")]
    pub exact_match: Option<bool>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct ExchangeConfig {
    #[serde(alias = "assetId")]
    pub asset_id: String,
    #[serde(alias = "authType")]
    pub auth_type: Option<String>,
    #[serde(
        alias = "baseUrl",
        default,
        deserialize_with = "pdk::serde::deserialize_service_opt"
    )]
    pub base_url: Option<pdk::hl::Service>,
    #[serde(alias = "credSecretRef")]
    pub cred_secret_ref: String,
    #[serde(alias = "groupId")]
    pub group_id: String,
    #[serde(alias = "orgId")]
    pub org_id: String,
    #[serde(alias = "refreshIntervalSec")]
    pub refresh_interval_sec: Option<i64>,
    #[serde(alias = "version")]
    pub version: String,
}
#[derive(Deserialize, Clone, Debug)]
pub struct FailOpenConfig {
    #[serde(alias = "onPinUnavailable")]
    pub on_pin_unavailable: Option<bool>,
}
#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(alias = "enforce")]
    pub enforce: Option<EnforceConfig>,
    #[serde(alias = "exchange")]
    pub exchange: ExchangeConfig,
    #[serde(alias = "failOpen")]
    pub fail_open: Option<FailOpenConfig>,
    #[serde(alias = "mode")]
    pub mode: Option<String>,
}
#[pdk::hl::entrypoint_flex]
fn init(abi: &dyn pdk::flex_abi::api::FlexAbi) -> Result<(), anyhow::Error> {
    let config: Config = serde_json::from_slice(abi.get_configuration())
        .map_err(|err| {
            anyhow::anyhow!(
                "Failed to parse configuration '{}'. Cause: {}",
                String::from_utf8_lossy(abi.get_configuration()), err
            )
        })?;
    let current = config.exchange;
    if current.base_url.is_some() {
        let service = current.base_url.unwrap();
        abi.service_create(service)?;
    }
    abi.setup()?;
    Ok(())
}
