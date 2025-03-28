
mod block_chain;
mod db;
mod routes;

use std::env;
use actix_cors::Cors;
use actix_web::{App, HttpServer,HttpResponse, post, web,Responder, get};
// main.rs
use teloxide::{prelude::*};
use dotenv::dotenv;
use reqwest::Url;
use teloxide::types::{ChatMemberKind, InlineKeyboardButton, InlineKeyboardMarkup};
use ethers::{
    prelude::*,
    utils::hash_message,
};
use ethers::utils::hex;
use reqwest::Client;
use std::str::FromStr;
use ethers::abi::Abi;
use std::sync::Arc;
use std::time::Duration;
use sqlx::{postgres::PgPoolOptions, PgPool};
use anyhow;
use std::collections::HashMap;
use chrono;
use crate::routes::signature::handle_verify;
use crate::routes::agent::{handle_add_tg_bot,get_agents,get_agent_by_name,get_agent_detail};
use crate::routes::user::get_user_shares_handler;
const ABI: &str = r#"[	{
		"inputs": [
			{
				"internalType": "address",
				"name": "",
				"type": "address"
			},
			{
				"internalType": "address",
				"name": "",
				"type": "address"
			}
		],
		"name": "sharesBalance",
		"outputs": [
			{
				"internalType": "uint256",
				"name": "",
				"type": "uint256"
			}
		],
		"stateMutability": "view",
		"type": "function"
	}]"#;

#[derive(Clone)]
struct AppConfig {
    telegram_bot_token: String,
    telegram_group_id: String,
    shares_contract: String,
    chain_rpc: String,
    database_url: String,
    start_block: u64,
}

use serde::{Deserialize, Serialize};
use sqlx::types::BigDecimal;
use crate::block_chain::sync::sync_trade_events;

#[tokio::main]
async fn main() {
    dotenv().ok();
    let config = AppConfig {
        telegram_bot_token: env::var("TELEGRAM_BOT_TOKEN")
            .expect("TELEGRAM_BOT_TOKEN not set"),
        telegram_group_id: env::var("TELEGRAM_GROUP_ID")
            .expect("TELEGRAM_GROUP_ID not set"),
        shares_contract: env::var("SHARES_CONTRACT_ADDRESS")
            .expect("SHARES_CONTRACT_ADDRESS not set"),
        chain_rpc: env::var("CHAIN_RPC")
            .expect("CHAIN_RPC not set"),
        database_url: env::var("DATABASE_URL")
            .expect("DATABASE_URL not set"),
        start_block: env::var("START_BLOCK")
            .expect("START_BLOCK not set")
            .parse()
            .expect("START_BLOCK must be a number"),
    };
    
    // Initialize database connection pool
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&config.database_url)
        .await
        .expect("Failed to connect to database");
    
    // Initialize database tables
    //init_db(&pool).await.expect("Failed to initialize database");
    
    let bots = sqlx::query!("SELECT bot_token FROM telegram_bots")
        .fetch_all(&pool)
        .await
        .expect("Failed to fetch existing bot configurations");
    
    for bot_record in bots {
        let bot_token = bot_record.bot_token;
        println!("Starting existing bot with token: {}", bot_token);
        
        tokio::spawn(async move {
            let bot = Bot::new(&bot_token);
            teloxide::repl(bot, |bot: Bot, msg: Message| async move {
                if let Some(new_chat_members) = msg.new_chat_members() {
                    for user in new_chat_members {
                        println!(
                            "[newChatMember] chat ID: {}, user ID: {}, user name: @{}",
                            msg.chat.id,
                            user.id,
                            user.username.as_deref().unwrap_or("nick user")
                        );
                        
                        let url_str = format!("http://127.0.0.1:8000/sign.html?challenge={}", user.id);
                        let url = Url::parse(&url_str).unwrap();
                        let keyboard = InlineKeyboardMarkup::new(
                            vec![vec![
                                InlineKeyboardButton::url(
                                    "ClickToSign",
                                     url,
                                )
                            ]]
                        );

                        bot.send_message(user.id, "Please sign to verify wallet ownership:")
                            .reply_markup(keyboard)
                            .await.unwrap();
                    }
                }

                if let Some(user) = msg.left_chat_member() {
                    println!(
                        "[MemberLeft] chat ID: {}, user ID: {}, user name: @{}",
                        msg.chat.id,
                        user.id,
                        user.username.as_deref().unwrap_or("nick user")
                    )
                }

                respond(())
            }).await;
        });
    }
    
    // Set up signal handler for graceful shutdown
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
    
    // Handle Ctrl+C signal
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                println!("Received Ctrl+C signal, shutting down gracefully...");
                let _ = shutdown_tx_clone.send(()).await;
            }
            Err(err) => {
                eprintln!("Error setting up Ctrl+C handler: {}", err);
            }
        }
    });
    
    let config_clone = config.clone();
    let pool_clone = pool.clone();
    let http_server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(cors)
            .app_data(web::Data::new(config_clone.clone()))
            .app_data(web::Data::new(pool_clone.clone()))
            .service(handle_verify)
            .service(handle_add_tg_bot)
            .service(get_agents)
            .service(get_agent_by_name)
            .service(get_agent_detail)
            .service(get_user_shares_handler)
    })
        .bind("0.0.0.0:8088").unwrap()
        .run();
    

    let bot = Bot::new(&config.telegram_bot_token);
    
    // Create futures for all main tasks
    let server_future = http_server;
    let sync_future = sync_trade_events(config, pool);
    
    // Run all tasks concurrently and terminate when either completes or shutdown signal received
    tokio::select! {
        _ = server_future => println!("HTTP server terminated"),
        _ = sync_future => println!("Blockchain sync process terminated"),
        _ = shutdown_rx.recv() => println!("Shutdown signal received, terminating all tasks"),
    }
    
    println!("Application shutdown complete");
}