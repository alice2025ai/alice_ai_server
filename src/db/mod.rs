pub mod models;
pub mod operations;

use sqlx::PgPool;

// 初始化数据库函数
pub async fn init_db(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS trades (
            trader VARCHAR NOT NULL,
            subject VARCHAR NOT NULL,
            share_amount NUMERIC NOT NULL DEFAULT 0,
            PRIMARY KEY (trader, subject)
        );
        CREATE TABLE IF NOT EXISTS user_mappings (
            address VARCHAR NOT NULL,
            telegram_id VARCHAR NOT NULL,
            is_banned BOOLEAN NOT NULL DEFAULT FALSE,
            PRIMARY KEY (address)
        );
        CREATE TABLE IF NOT EXISTS sync_status (
            id SERIAL PRIMARY KEY,
            last_synced_block BIGINT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS telegram_bots (
            agent_name VARCHAR NOT NULL PRIMARY KEY,
            bio TEXT,
            invite_url VARCHAR(128) NOT NULL,
            bot_token VARCHAR NOT NULL,
            chat_group_id VARCHAR NOT NULL,
            subject_address VARCHAR NOT NULL,
            created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "
    )
    .execute(pool)
    .await?;
    
    Ok(())
}
