use serde::{Deserialize, Serialize};
use typeshare::typeshare;

use crate::WalletId;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[typeshare(swift = "Equatable, Sendable")]
#[serde(rename_all = "camelCase")]
pub struct WalletConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_multi_signature_accounts: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[typeshare(swift = "Equatable, Sendable")]
#[serde(rename_all = "camelCase")]
pub struct WalletConfigurationResult {
    pub wallet_id: WalletId,
    pub configuration: WalletConfiguration,
}
