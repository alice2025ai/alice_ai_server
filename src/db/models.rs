use serde::{Deserialize, Serialize};
use sqlx::types::BigDecimal;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub telegram_bot_token: String,
    pub telegram_group_id: String,
    pub shares_contract: String,
    pub chain_rpc: String,
    pub database_url: String,
    pub start_block: u64,
}

#[derive(Clone, Debug)]
pub struct UserShares {
    pub trader: String,
    pub subject: String,
    pub share_amount: BigDecimal,
}

#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub challenge: String,
    pub signature: String,
    pub shares_subject: String,
    pub user: String,
}

#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}