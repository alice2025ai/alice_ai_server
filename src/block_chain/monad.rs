use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use ethers::{
    prelude::*,
    contract::Contract,
};
use ethers::utils::{hash_message, hex};
use sqlx::types::BigDecimal;
use sqlx::PgPool;
use reqwest::Client;
use teloxide::Bot;
use teloxide::prelude::{Requester, UserId};
use teloxide::types::ChatPermissions;
use anyhow::{Result, anyhow};
use async_trait::async_trait;

use crate::block_chain::Blockchain;
use crate::block_chain::utils::{TradeEvent, TRADE_ABI, ABI};
use crate::db::operations::{get_last_synced_block, process_buy_trade, process_sell_trade, update_last_synced_block};
use crate::AppConfig;

/// Monad区块链实现
pub struct MonadBlockchain {
    provider: Arc<Provider<Http>>,
    contract_address: Address,
    config: Arc<AppConfig>,
}

impl MonadBlockchain {
    pub fn new(config: Arc<AppConfig>) -> Self {
        let provider = Provider::<Http>::try_from(&config.chain_rpc).expect("Failed to connect to blockchain node");
        let provider = Arc::new(provider);
        
        let contract_address = Address::from_str(&config.shares_contract).expect("Invalid contract address");
        
        Self {
            provider,
            contract_address,
            config,
        }
    }
    
    /// 处理交易事件
    async fn process_trade_event(&self, event: &TradeEvent, pool: &sqlx::PgPool) -> Result<()> {
        println!("Processing Monad Trade event: {:?}", event);
        
        let client = Client::new();
        let share_amount = BigDecimal::from_str(&event.share_amount.to_string())?;
        let trader = hex::encode(event.trader.as_bytes());
        let subject = hex::encode(event.subject.as_bytes());
        
        if event.is_buy {
            // 买入操作，增加份额
            process_buy_trade(
                pool, 
                trader.clone(),
                subject.clone(),
                share_amount,
                self.get_name(),
            ).await?;
            
            // 检查用户是否处于禁止状态
            let user_mapping = sqlx::query!(
                "SELECT telegram_id, is_banned FROM user_mappings WHERE address = $1 AND chain_type = $2",
                trader.clone(), 
                self.get_name()
            )
            .fetch_optional(pool)
            .await?;
            
            if let Some(user) = user_mapping {
                if user.is_banned {
                    let user_share = sqlx::query!(
                        "SELECT share_amount FROM trades WHERE trader = $1 AND subject = $2 AND chain_type = $3",
                        trader.clone(),
                        subject.clone(),
                        self.get_name()
                    )
                    .fetch_optional(pool)
                    .await?;
                    
                    if let Some(share) = user_share {
                        if share.share_amount > BigDecimal::from(0) {
                            let bot_info = sqlx::query!(
                                "SELECT bot_token, chat_group_id FROM telegram_bots WHERE subject_address = $1 AND chain_type = $2",
                                subject.clone(),
                                self.get_name()
                            )
                            .fetch_optional(pool)
                            .await?;
                            
                            if let Some(bot_info) = bot_info {
                                let permissions = ChatPermissions::empty()
                                    | ChatPermissions::SEND_MESSAGES
                                    | ChatPermissions::SEND_MEDIA_MESSAGES
                                    | ChatPermissions::SEND_OTHER_MESSAGES
                                    | ChatPermissions::SEND_POLLS
                                    | ChatPermissions::ADD_WEB_PAGE_PREVIEWS;

                                let bot = Bot::new(bot_info.bot_token);
                                let user_id: u64 = user.telegram_id.parse().unwrap();
                                bot.restrict_chat_member(bot_info.chat_group_id, UserId(user_id), permissions).await?;
                            }
                        }
                    }
                }
            }
        } else {
            // 卖出操作，减少份额
            println!("Trader {} sell {} shares of subject {}", trader, share_amount, subject);
            let (should_ban, telegram_id_opt) = process_sell_trade(
                pool,
                trader.clone(),
                subject.clone(),
                share_amount,
                self.get_name(),
            ).await?;
            
            if should_ban {
                if let Some(telegram_id) = telegram_id_opt {
                    println!("User {} has 0 shares for {}, banning user", &trader, &subject);
                    
                    // Get the bot token and chat group id from telegram_bots table for this subject
                    let bot_info = sqlx::query!(
                        "SELECT bot_token, chat_group_id FROM telegram_bots WHERE subject_address = $1 AND chain_type = $2",
                        subject.clone(),
                        self.get_name()
                    )
                    .fetch_optional(pool)
                    .await?;
                    
                    if let Some(bot_info) = bot_info {
                        let permissions = ChatPermissions::empty();

                        let bot = Bot::new(bot_info.bot_token);
                        let user_id: u64 = telegram_id.parse().unwrap();
                        bot.restrict_chat_member(bot_info.chat_group_id, UserId(user_id), permissions).await?;
                        sqlx::query!(
                            "UPDATE user_mappings SET is_banned = true WHERE address = $1 AND chain_type = $2",
                            trader.clone(),
                            self.get_name()
                        )
                        .execute(pool)
                        .await?;
                    } else {
                        println!("No telegram bot info found for subject {}", &subject);
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Blockchain for MonadBlockchain {
    fn get_name(&self) -> &'static str {
        "monad"
    }
    
    async fn sync_events(&self, pool: &PgPool) -> Result<()> {
        let contract_address = self.contract_address;
        let provider = self.provider.clone();
        
        let abi: ethers::abi::Abi = serde_json::from_str(TRADE_ABI).expect("Invalid ABI");
        let contract = Contract::new(contract_address, abi, provider.clone());
        
        // 获取最后同步的区块号
        let mut last_synced_block = get_last_synced_block(pool, self.config.start_block, self.get_name()).await?;
        
        println!("Starting sync from block {} for {}", last_synced_block, self.get_name());
        
        // 批量同步的区块间隔
        const BLOCK_BATCH_SIZE: u64 = 100;
        
        loop {
            // 获取当前链上最新区块
            let current_block = match provider.get_block_number().await {
                Ok(block) => block.as_u64(),
                Err(e) => {
                    println!("Failed to get current block number: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            
            if last_synced_block >= current_block {
                // 已经同步到最新区块，等待一段时间后继续
                println!("Synced to current block {} for {}, waiting for new blocks...", current_block, self.get_name());
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
            
            // 计算本次同步的结束区块
            let end_block = std::cmp::min(last_synced_block + BLOCK_BATCH_SIZE, current_block);
            
            println!("Syncing blocks {} to {} for {}", last_synced_block, end_block, self.get_name());
            
            // 创建过滤器查询历史事件
            let filter = contract
                .event::<TradeEvent>()
                .from_block(last_synced_block)
                .to_block(end_block);
            
            // 查询事件
            match filter.query().await {
                Ok(events) => {
                    println!("Found {} events in blocks {} to {} for {}", events.len(), last_synced_block, end_block, self.get_name());
                    
                    // 处理每个事件
                    for event in events {
                        if let Err(e) = self.process_trade_event(&event, pool).await {
                            println!("Error processing trade event: {:?}", e);
                        }
                    }
                    
                    // 更新最后同步的区块号
                    if let Err(e) = update_last_synced_block(pool, end_block, self.get_name()).await {
                        println!("Failed to update last synced block: {:?}", e);
                    } else {
                        last_synced_block = end_block;
                    }
                },
                Err(e) => {
                    println!("Failed to query events: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
            
            // 短暂休息，避免请求过于频繁
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    
    fn verify_signature(&self, challenge: &str, signature: &str) -> Result<String, String> {
        let sig_bytes = hex::decode(signature)
            .map_err(|e| format!("Invalid signature hex: {}", e))?;

        if sig_bytes.len() != 65 {
            return Err("Signature must be 65 bytes".into());
        }

        let message_hash = hash_message(challenge);
        let signature = Signature::try_from(sig_bytes.as_slice())
            .map_err(|e| format!("Invalid signature: {}!", e))?;
        let recovered_address = signature
            .recover(message_hash)
            .map_err(|e| format!("Recovery failed: {}", e))?;
        
        Ok(hex::encode(recovered_address.as_bytes()))
    }
    
    async fn get_shares_balance(&self, subject: &str, user: &str) -> Result<u64> {
        let subject_address = Address::from_str(subject).map_err(|e| anyhow!("Invalid subject address: {}", e))?;
        let user_address = Address::from_str(user).map_err(|e| anyhow!("Invalid user address: {}", e))?;
        
        let abi: ethers::abi::Abi = serde_json::from_str(ABI).expect("Invalid abi");
        let contract = ethers::contract::Contract::new(
            self.contract_address,
            abi,
            self.provider.clone()
        );

        let balance: U256 = contract
            .method::<_, U256>("sharesBalance", (subject_address, user_address))
            .map_err(|e| anyhow!("Failed to get sharesBalance method: {}", e))?
            .call()
            .await
            .map_err(|e| anyhow!("Failed to call sharesBalance: {}", e))?;
            
        Ok(balance.as_u64())
    }
}

// 批量同步历史事件，适配原始接口
pub async fn sync_trade_events(config: AppConfig, pool: sqlx::PgPool) {
    let config_arc = Arc::new(config);
    
    // 创建需要同步的链任务
    let mut sync_tasks = Vec::new();
    
    // 根据特性标志决定是否启动Monad链同步
    #[cfg(feature = "monad")]
    {
        let monad = MonadBlockchain::new(config_arc.clone());
        sync_tasks.push(Box::pin(async move {
            if let Err(e) = monad.sync_events(&pool).await {
                println!("Error syncing Monad events: {:?}", e);
            }
        }));
    }
    
    // 根据特性标志决定是否启动Sui链同步
    #[cfg(feature = "sui")]
    {
        let sui = crate::block_chain::sui::SuiBlockchain::new(config_arc.clone());
        sync_tasks.push(Box::pin(async move {
            if let Err(e) = sui.sync_events(&pool).await {
                println!("Error syncing Sui events: {:?}", e);
            }
        }));
    }
    
    // 并发执行所有启用的链同步任务
    futures::future::join_all(sync_tasks).await;
} 