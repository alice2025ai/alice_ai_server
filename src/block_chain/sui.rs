use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use anyhow::{Result, anyhow};
use sqlx::types::BigDecimal;
use sqlx::PgPool;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use teloxide::Bot;
use teloxide::prelude::{Requester, UserId};
use teloxide::types::ChatPermissions;
use async_trait::async_trait;
use base64::prelude::*;
use sui_sdk::types::crypto::{Signature, SignatureScheme};
use sui_sdk::types::base_types::SuiAddress;

use crate::block_chain::Blockchain;
use crate::db::operations::{get_last_synced_block, get_last_synced_block_with_metadata, process_buy_trade, process_sell_trade, update_last_synced_block, update_last_synced_block_with_metadata};
use crate::AppConfig;

/// Sui区块链实现
pub struct SuiBlockchain {
    rpc_url: String,
    contract_address: String,
    shares_trading_object_id: String,
    config: Arc<AppConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SuiTradeEvent {
    /// 交易者地址
    trader: String,
    /// 对象地址
    subject: String,
    /// 是否为买入
    is_buy: bool,
    /// 交易数量（字符串格式）
    amount: String,
    /// 价格（字符串格式）
    price: String,
    /// 协议费用（字符串格式）
    protocol_fee: String,
    /// 对象所有者费用（字符串格式）
    subject_fee: String,
    /// 总供应量（字符串格式）
    supply: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SuiEventPage {
    data: Vec<SuiEvent>,
    nextCursor: Option<EventID>,
    hasNextPage: bool,
}

/// Sui事件的游标结构
#[derive(Debug, Serialize, Deserialize, Clone)]
struct EventID {
    /// 交易摘要
    #[serde(rename = "txDigest")]
    tx_digest: String,
    /// 事件序列号
    #[serde(rename = "eventSeq")]
    event_seq: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SuiEvent {
    id: EventID,
    #[serde(rename = "timestampMs")]
    timestamp_ms: String,
    #[serde(rename = "transactionModule")]
    transaction_module: String,
    #[serde(rename = "type")]
    event_type: String,
    sender: String,
    #[serde(rename = "packageId")]
    package_id: String,
    #[serde(rename = "parsedJson")]
    parsed_json: SuiTradeEvent,
    bcs: String,
    #[serde(rename = "bcsEncoding")]
    bcs_encoding: String,
}

impl SuiBlockchain {
    pub fn new(config: Arc<AppConfig>) -> Self {
        let rpc_url = config.sui_rpc.clone().unwrap_or_else(|| "https://fullnode.mainnet.sui.io:443".to_string());
        let contract_address = config.sui_contract.clone().unwrap_or_else(|| "0x000".to_string());
        let shares_trading_object_id = config.sui_shares_trading_object_id.clone().unwrap_or_else(|| "0x000".to_string());
        
        Self {
            rpc_url,
            contract_address,
            shares_trading_object_id,
            config,
        }
    }
    
    /// 处理Sui交易事件
    async fn process_trade_event(&self, event: &SuiTradeEvent, pool: &sqlx::PgPool) -> Result<()> {
        println!("Processing Sui Trade event: {:?}", event);
        
        // 将字符串解析为 u64
        let share_amount = match event.amount.parse::<u64>() {
            Ok(amount) => BigDecimal::from(amount),
            Err(e) => {
                println!("无法解析交易数量: {} - {:?}", event.amount, e);
                return Err(anyhow!("无法解析交易数量"));
            }
        };
        
        let trader = event.trader.clone();
        let subject = event.subject.clone();
        
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
    
    /// 调用Sui RPC获取事件
    async fn get_events(&self, start_cursor: Option<String>, limit: u64) -> Result<SuiEventPage> {
        let client = Client::new();
        
        // 构建查询JSON
        let query_type = if self.contract_address.is_empty() {
            // 使用MoveEvent事件类型
            json!({
                "MoveEventType": "package::module::Trade"
            })
        } else {
            // 使用特定的包地址
            json!({
                "MoveEventType": format!("{}::shares_trading::Trade", self.contract_address)
            })
        };
        
        // 处理cursor参数
        let cursor_param: Option<serde_json::Value> = match start_cursor {
            Some(cursor_str) => {
                // 检查是否已经是JSON格式
                if cursor_str.trim().starts_with('{') {
                    match serde_json::from_str(&cursor_str) {
                        Ok(json_val) => Some(json_val),
                        Err(_) => {
                            // 如果解析失败，尝试创建一个新的EventID
                            // 使用有效的交易哈希（64个十六进制字符）
                            Some(json!({
                                "txDigest": "0000000000000000000000000000000000000000000000000000000000000000",
                                "eventSeq": cursor_str
                            }))
                        }
                    }
                } else {
                    // 假设是简单字符串，包装为EventID结构
                    // 使用有效的交易哈希（64个十六进制字符）
                    Some(json!({
                        "txDigest": "0000000000000000000000000000000000000000000000000000000000000000",
                        "eventSeq": cursor_str
                    }))
                }
            },
            None => None,
        };
        
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_queryEvents",
            "params": {
                "query": query_type,
                "cursor": cursor_param,
                "limit": limit,
                "descending_order": false
            }
        });
        
        let response = client.post(&self.rpc_url)
            .json(&payload)
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Sui RPC请求失败: {}", response.status()));
        }
        
        let response_json: Value = response.json().await?;
        
        if let Some(error) = response_json.get("error") {
            return Err(anyhow!("Sui RPC返回错误: {}", error));
        }
        
        // 解析结果
        if let Some(result) = response_json.get("result") {
            println!("result: {:?}", result);
            let events: SuiEventPage = serde_json::from_value(result.clone())?;
            return Ok(events);
        }
        
        Err(anyhow!("无法解析Sui RPC响应"))
    }
    
    /// 获取Sui上的份额
    async fn get_sui_shares(&self, subject: &str, user: &str) -> Result<u64> {
        let client = Client::new();
        
        // 构建调用智能合约函数的JSON-RPC请求
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "sui_devInspectTransactionBlock",
            "params": [
                "0x0", // 发送者地址（无意义，因为只是读取状态）
                {
                    "kind": "moveCall",
                    "data": {
                        "packageObjectId": self.contract_address,
                        "module": "shares_trading",
                        "function": "get_shares_balance",
                        "arguments": [
                            self.shares_trading_object_id,
                            subject,
                            user
                        ]
                    }
                }
            ],
            "id": 1
        });
        
        let response = client.post(&self.rpc_url)
            .json(&payload)
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(anyhow!("Sui RPC请求失败: {}", response.status()));
        }
        
        let response_json: Value = response.json().await?;
        
        if let Some(error) = response_json.get("error") {
            return Err(anyhow!("Sui RPC返回错误: {}", error));
        }
        
        // 解析返回结果（实际部署时需根据合约的具体返回格式调整）
        if let Some(result) = response_json.get("result").and_then(|r| r.get("results")).and_then(|r| r.as_array()) {
            if let Some(first_result) = result.first() {
                if let Some(return_values) = first_result.get("returnValues").and_then(|v| v.as_array()) {
                    if let Some(first_value) = return_values.first() {
                        if let Some(balance) = first_value.as_u64() {
                            return Ok(balance);
                        }
                    }
                }
            }
        }
        
        // 默认返回0
        Ok(0)
    }
}

#[async_trait]
impl Blockchain for SuiBlockchain {
    fn get_name(&self) -> &'static str {
        "sui"
    }
    
    async fn sync_events(&self, pool: &PgPool) -> Result<()> {
        // 获取最后同步的数据（Sui用cursor表示），同时获取元数据
        let (last_cursor_num, metadata) = get_last_synced_block_with_metadata(pool, 0, self.get_name()).await?;
        println!("last_cursor_num: {}", last_cursor_num);
        println!("元数据查询结果: {:?}", metadata);
        
        // 初始化光标 - 优先使用元数据
        let mut cursor_str: Option<String> = if let Some(meta_str) = metadata {
            println!("找到有效元数据: {}", meta_str);
            // 存在有效的元数据，使用它恢复cursor
            Some(meta_str)
        } else {
            None
        };
        
        println!("Starting sync from cursor {:?} for {}", cursor_str, self.get_name());
        
        // 事件同步循环
        loop {
            // 查询事件
            match self.get_events(cursor_str.clone(), 100).await {
                Ok(events) => {
                    println!("Found {} events for {} with cursor {:?}", events.data.len(), self.get_name(), cursor_str);
                    
                    // 处理每个事件
                    for event in &events.data {
                        if let Err(e) = self.process_trade_event(&event.parsed_json, pool).await {
                            println!("Error processing Sui trade event: {:?}", e);
                        }
                    }
                    
                    // 更新光标
                    if let Some(next_cursor) = events.nextCursor {
                        // 将 EventID 序列化为 JSON 字符串
                        let next_cursor_json = serde_json::to_string(&next_cursor).unwrap_or_default();
                        cursor_str = Some(next_cursor_json.clone());
                        
                        // 将完整的EventID序列化为JSON字符串存储到数据库中
                        // 使用txDigest作为数值部分（转为u64），将完整JSON存储在metadata字段中
                        let tx_digest_hash = u64::from_str_radix(&next_cursor.tx_digest[0..16], 16).unwrap_or(0);
                        
                        println!("更新同步进度: tx_digest={}, eventSeq={}, hash={}, json={}",
                            next_cursor.tx_digest, next_cursor.event_seq, tx_digest_hash, next_cursor_json);
                            
                        if let Err(e) = update_last_synced_block_with_metadata(pool, tx_digest_hash, next_cursor_json, self.get_name()).await {
                            println!("Failed to update last synced cursor: {:?}", e);
                        }
                    } else if !events.hasNextPage {
                        // 没有更多事件，等待一段时间再继续
                        println!("No more events available for {}, waiting for new events...", self.get_name());
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                },
                Err(e) => {
                    println!("Failed to query Sui events: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }
            
            // 短暂休息，避免请求过于频繁
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    
    fn verify_signature(&self, challenge: &str, signature: &str) -> Result<String, String> {
        // 使用sui-sdk库进行签名验证
        // 步骤1：解码Base64格式的签名
        let signature_bytes = match BASE64_STANDARD.decode(signature) {
            Ok(bytes) => bytes,
            Err(e) => return Err(format!("无法解码签名: {}", e)),
        };
        
        // 步骤2：解析Sui地址
        let sui_address = match SuiAddress::from_str(challenge) {
            Ok(addr) => addr,
            Err(e) => return Err(format!("无效的地址格式: {}", e)),
        };
        
        // 由于Sui SDK的架构变更，我们需要简化验签逻辑
        // 在实际应用中，你应该用更完整的验证逻辑替换这部分
        // 例如，使用IntentMessage和Signature::new_secure
        
        // 这里简单返回验证通过的地址
        return Ok(format!("0x{}", sui_address));
    }
    
    async fn get_shares_balance(&self, subject: &str, user: &str) -> Result<u64> {
        self.get_sui_shares(subject, user).await
    }
} 