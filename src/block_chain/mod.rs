pub mod monad;
pub mod utils;
pub mod sui;

use anyhow::Result;
use sqlx::PgPool;
use std::sync::Arc;
use async_trait::async_trait;

/// 区块链接口抽象
#[async_trait]
pub trait Blockchain: Send + Sync {
    /// 获取区块链名称
    fn get_name(&self) -> &'static str;
    
    /// 同步交易事件
    async fn sync_events(&self, pool: &PgPool) -> Result<()>;
    
    /// 验证用户签名
    fn verify_signature(&self, challenge: &str, signature: &str) -> Result<String, String>;
    
    /// 获取用户持有的份额
    async fn get_shares_balance(&self, subject: &str, user: &str) -> Result<u64>;
}

// 工厂函数创建不同链的实现
pub fn create_blockchain(chain_type: &str, config: Arc<crate::AppConfig>) -> Box<dyn Blockchain> {
    match chain_type {
        "monad" => Box::new(monad::MonadBlockchain::new(config)),
        "sui" => Box::new(sui::SuiBlockchain::new(config)),
        _ => panic!("不支持的区块链类型: {}", chain_type),
    }
} 