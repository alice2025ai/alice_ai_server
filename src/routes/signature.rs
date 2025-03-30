use std::sync::Arc;
use actix_web::{HttpResponse, post, Responder, web};
use ethers::addressbook::Address;
use ethers::prelude::{Http, Provider, Signature, U256};
use ethers::utils::{hash_message, hex};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use crate::{ABI, AppConfig};
use std::str::FromStr;
use teloxide::Bot;
use teloxide::prelude::{Requester, UserId};
use teloxide::types::{ChatPermissions, ChatMemberStatus, Message};

#[derive(Debug, Deserialize)]
pub struct ChallengeRequest {
    pub challenge: String,
    pub chat_id: String,
    pub signature: String,
    pub user: String,
}

#[derive(Debug, Serialize)]
pub struct ChallengeResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
pub fn verify_signature(
    challenge: &str,
    signature: &str,
) -> Result<Address, String> {
    let sig_bytes = hex::decode(signature)
        .map_err(|e| format!("Invalid signature hex: {}", e))?;

    if sig_bytes.len() != 65 {
        return Err("Signature must be 65 bytes".into());
    }

    let message_hash = hash_message(challenge);
    let signature = Signature::try_from(sig_bytes.as_slice()).map_err(|e| format!("Invalid signature: {}!",e))?;
    let recovered_address = signature
        .recover(message_hash)
        .map_err(|e| format!("Recovery failed: {}", e))?;
    Ok(recovered_address)
}


#[post("/verify-signature")]
async fn handle_verify(
    data: web::Json<ChallengeRequest>,
    config: web::Data<AppConfig>,
    pool: web::Data<PgPool>,
) -> impl Responder {

    // Query bot info including subject_address from telegram_bots table using chat_id
    let bot_info = match sqlx::query!(
        "SELECT bot_token, chat_group_id, subject_address FROM telegram_bots WHERE chat_group_id = $1",
        data.chat_id
    )
    .fetch_optional(pool.get_ref())
    .await {
        Ok(Some(info)) => info,
        Ok(None) => {
            println!("No bot info found for chat_id: {}", data.chat_id);
            return HttpResponse::BadRequest().json(ChallengeResponse {
                success: false,
                error: Some("Bot not found for this chat_id".into()),
            });
        },
        Err(e) => {
            println!("Failed to query bot info: {:?}", e);
            return HttpResponse::InternalServerError().json(ChallengeResponse {
                success: false,
                error: Some(format!("Database query failed: {}", e)),
            });
        }
    };

    let own_shares = match verify_signature(
        &data.challenge,
        // &data.address,
        &data.signature,
    ) {
        Ok(address) => {
            println!("Verified address is {}",address.to_string());
            let user_address = Address::from_str(&data.user).expect("Invalid user address");
            if user_address == address {
                // When address matches, save user address and Telegram ID to database
                let user_address_str = hex::encode(user_address.as_bytes());
                let telegram_id = &data.challenge;

                // Check if user address already exists
                //todo: User should be able to unbind/update current address or Telegram
                let result = sqlx::query!(
                    "INSERT INTO user_mappings (address, telegram_id)
                     VALUES ($1, $2)
                     ON CONFLICT (address) DO NOTHING",
                    user_address_str,
                    telegram_id
                )
                    .execute(pool.get_ref())
                    .await;

                if let Err(e) = result {
                    println!("Failed to save user mapping: {:?}", e);
                }

                let provider = Provider::<Http>::try_from(&config.chain_rpc).expect("Connect monad failed");
                let contract_address = Address::from_str(&config.shares_contract).expect("Invalid contract");
                let abi: ethers::abi::Abi = serde_json::from_str(ABI).expect("Invalid abi");
                let contract = ethers::contract::Contract::new(
                    contract_address,
                    abi,
                    Arc::new(provider)
                );

                // Use subject_address from bot_info instead of request
                let subject_address = Address::from_str(&bot_info.subject_address).expect("Invalid subject address");

                let balance: U256 = contract
                    .method::<_, U256>("sharesBalance", (subject_address, user_address)).expect("Get method failed")
                    .call()
                    .await.expect("Call sharesBalance failed");

                println!("Balance: {}", balance);
                !balance.is_zero()
            } else {
                println!("Address mismatch with signature!");
                false
            }
        }
        Err(e) => {
            println!("Verify signature failed: {:?}",e);
            false
        },
    };
    if own_shares {
        let permissions = ChatPermissions::empty()
            | ChatPermissions::SEND_MESSAGES
            | ChatPermissions::SEND_MEDIA_MESSAGES
            | ChatPermissions::SEND_OTHER_MESSAGES
            | ChatPermissions::SEND_POLLS
            | ChatPermissions::ADD_WEB_PAGE_PREVIEWS;

        let bot = Bot::new(bot_info.bot_token);
        let user_id: u64 = data.challenge.parse().unwrap();
        match bot.restrict_chat_member(bot_info.chat_group_id, UserId(user_id), permissions).await {
            Ok(_) => {
                return HttpResponse::Ok().json(ChallengeResponse {
                    success: true,
                    error: None,
                });
            }
            Err(e) => {
                println!(" restrict_chat_member failed: {:?}",e);
                return HttpResponse::InternalServerError().json(ChallengeResponse {
                    success: false,
                    error: Some(format!("Telegram restrict_chat_member failed: {}", e)),
                });
            },
        }
    }

    HttpResponse::Ok().json(ChallengeResponse {
        success: true,
        error: None,
    })
}