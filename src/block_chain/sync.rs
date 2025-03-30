use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use actix_web::HttpResponse;
use ethers::{
    prelude::*,
    contract::Contract,
};
use ethers::utils::hex;
use sqlx::types::BigDecimal;
use reqwest::Client;
use teloxide::Bot;
use teloxide::prelude::{Requester, UserId};
use teloxide::types::ChatPermissions;
use crate::{AppConfig};

use crate::block_chain::utils::{TradeEvent, TRADE_ABI};
use crate::db::operations::{get_last_synced_block, process_buy_trade, process_sell_trade, update_last_synced_block};
use crate::routes::signature::ChallengeResponse;

// 批量同步历史事件
pub async fn sync_trade_events(config: AppConfig, pool: sqlx::PgPool) {
    let provider = Provider::<Http>::try_from(&config.chain_rpc).expect("Failed to connect to blockchain node");
    let provider = Arc::new(provider);
    
    let contract_address = Address::from_str(&config.shares_contract).expect("Invalid contract address");
    let abi: ethers::abi::Abi = serde_json::from_str(TRADE_ABI).expect("Invalid ABI");
    
    let contract = Contract::new(contract_address, abi, provider.clone());
    
    // 获取最后同步的区块号
    let mut last_synced_block = get_last_synced_block(&pool, config.start_block).await
        .expect("Failed to get last synced block");
    
    println!("Starting sync from block {}", last_synced_block);
    
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
            println!("Synced to current block {}, waiting for new blocks...", current_block);
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }
        
        // 计算本次同步的结束区块
        let end_block = std::cmp::min(last_synced_block + BLOCK_BATCH_SIZE, current_block);
        
        println!("Syncing blocks {} to {}", last_synced_block, end_block);
        
        // 创建过滤器查询历史事件
        let filter = contract
            .event::<TradeEvent>()
            .from_block(last_synced_block)
            .to_block(end_block);
        
        // 查询事件
        match filter.query().await {
            Ok(events) => {
                println!("Found {} events in blocks {} to {}", events.len(), last_synced_block, end_block);
                
                // 处理每个事件
                for event in events {
                    process_trade_event(&event, &pool, &config).await;
                }
                
                // 更新最后同步的区块号
                if let Err(e) = update_last_synced_block(&pool, end_block).await {
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

// 处理交易事件
async fn process_trade_event(event: &TradeEvent, pool: &sqlx::PgPool, config: &AppConfig) -> anyhow::Result<()> {
    println!("Processing Trade event: {:?}", event);
    
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
            share_amount
        ).await?;
        
        // 检查用户是否处于禁止状态
        let user_mapping = sqlx::query!(
            "SELECT telegram_id, is_banned FROM user_mappings WHERE address = $1",trader.clone()
        )
        .fetch_optional(pool)
        .await?;
        
        if let Some(user) = user_mapping {
            if user.is_banned {
                let user_share = sqlx::query!(
                    "SELECT share_amount FROM trades WHERE trader = $1 AND subject = $2",
                    trader.clone(),
                    subject.clone()
                )
                .fetch_optional(pool)
                .await?;
                
                if let Some(share) = user_share {
                    if share.share_amount > BigDecimal::from(0) {
                        let bot_info = sqlx::query!(
                            "SELECT bot_token, chat_group_id FROM telegram_bots WHERE subject_address = $1",
                            subject.clone()
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
        println!("Trader {} sell {} shares of subject {}",trader,share_amount,subject);
        let (should_ban, telegram_id_opt) = process_sell_trade(
            pool,
            trader.clone(),
            subject.clone(),
            share_amount
        ).await?;
        
        if should_ban {
            if let Some(telegram_id) = telegram_id_opt {
                println!("User {} has 0 shares for {}, banning user", &trader, &subject);
                
                // Get the bot token and chat group id from telegram_bots table for this subject
                let bot_info = sqlx::query!(
                    "SELECT bot_token, chat_group_id FROM telegram_bots WHERE subject_address = $1",
                    subject.clone()
                )
                .fetch_optional(pool)
                .await?;
                
                if let Some(bot_info) = bot_info {
                    let permissions = ChatPermissions::empty();

                    let bot = Bot::new(bot_info.bot_token);
                    let user_id: u64 = telegram_id.parse().unwrap();
                    bot.restrict_chat_member(bot_info.chat_group_id, UserId(user_id), permissions).await?;
                    sqlx::query!(
                        "UPDATE user_mappings SET is_banned = true WHERE address = $1",
                        trader.clone()
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