use sqlx::{PgPool, types::BigDecimal};
use std::str::FromStr;
use ethers::prelude::*;
use anyhow;
use crate::db::models::UserShares;

// Get the last synchronized block number
pub async fn get_last_synced_block(pool: &PgPool, start_block: u64) -> Result<u64, sqlx::Error> {
    let record = sqlx::query!(
        "SELECT last_synced_block FROM sync_status ORDER BY id DESC LIMIT 1"
    )
    .fetch_optional(pool)
    .await?;
    
    match record {
        Some(row) => Ok(row.last_synced_block as u64),
        None => {
            // If no record exists, insert the initial block number
            sqlx::query!(
                "INSERT INTO sync_status (last_synced_block) VALUES ($1)",
                start_block as i64
            )
            .execute(pool)
            .await?;
            
            Ok(start_block)
        }
    }
}

// Update the last synchronized block number
pub async fn update_last_synced_block(pool: &PgPool, block_number: u64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE sync_status SET last_synced_block = $1 WHERE id = (SELECT id FROM sync_status ORDER BY id DESC LIMIT 1)",
        block_number as i64
    )
    .execute(pool)
    .await?;
    
    Ok(())
}

// Process buy trade
pub async fn process_buy_trade(
    pool: &PgPool, 
    trader: String, 
    subject: String, 
    share_amount: BigDecimal
) -> anyhow::Result<()> {
    sqlx::query!(
        "INSERT INTO trades (trader, subject, share_amount) 
        VALUES ($1, $2, $3) 
        ON CONFLICT (trader, subject) 
        DO UPDATE SET share_amount = trades.share_amount + $3",
        trader,
        subject,
        share_amount,
    )
    .execute(pool)
    .await?;
    
    Ok(())
}

// Process sell trade
pub async fn process_sell_trade(
    pool: &PgPool, 
    trader: String, 
    subject: String, 
    share_amount: BigDecimal
) -> anyhow::Result<(bool, Option<String>)> {
    let ret = sqlx::query!(
        "UPDATE trades SET share_amount = share_amount - $1 
        WHERE trader = $2 AND subject = $3 
        RETURNING share_amount",
        share_amount,
        trader,
        subject
    )
    .fetch_optional(pool)
    .await?;
    
    match ret {
        Some(record) => {
            // Check if share_amount is 0
            if record.share_amount == 0.into() {
                // Get user's Telegram ID
                let telegram_id = sqlx::query!(
                    "SELECT telegram_id FROM user_mappings WHERE address = $1",
                    trader
                )
                .fetch_optional(pool)
                .await?;
                
                if let Some(user_record) = telegram_id {
                    return Ok((true, Some(user_record.telegram_id)));
                }
            }
            Ok((false, None))
        },
        None => {
            println!("Trade record not found: trader={}, subject={}", trader, subject);
            Ok((false, None))
        }
    }
}

// Get user's shares for a subject
pub async fn get_user_subject_shares(
    pool: &PgPool,
    trader: &str,
    subject: &str
) -> Result<BigDecimal, sqlx::Error> {
    let record = sqlx::query!(
        "SELECT share_amount FROM trades WHERE trader = $1 AND subject = $2",
        trader,
        subject
    )
    .fetch_optional(pool)
    .await?;
    
    match record {
        Some(row) => Ok(row.share_amount),
        None => Ok(BigDecimal::from_str("0").unwrap())
    }
}

pub async fn get_user_shares(
    pool: &PgPool,
    trader: &str,
) -> Result<Vec<UserShares>, sqlx::Error> {
    let rows = sqlx::query_as!(
        UserShares,
        "SELECT trader, subject, share_amount FROM trades WHERE trader = $1",
        trader,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows)
}