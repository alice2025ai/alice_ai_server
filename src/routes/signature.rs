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

                let subject_address = Address::from_str(&data.shares_subject).expect("Invalid subject address");

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
    if !own_shares {
        let client = Client::new();
        let url = format!(
            "https://api.telegram.org/bot{}/banChatMember",
            config.telegram_bot_token
        );
        let params = [
            ("chat_id", &config.telegram_group_id),
            ("user_id", &data.challenge),
        ];

        println!("url is {},params is {:?}",url,params);
        match client.post(&url).form(&params).send().await {
            Ok(resp) => {
                println!("resp is {:?}",resp.status());
                if !resp.status().is_success() {
                    return HttpResponse::InternalServerError().json(ChallengeResponse {
                        success: false,
                        error: Some(format!("Telegram API call failed",)),
                    });
                }
            }
            Err(e) => {
                println!("Verified signature failed: {:?}",e);
                return HttpResponse::InternalServerError().json(ChallengeResponse {
                    success: false,
                    error: Some(format!("Telegram request failed: {}", e)),
                });
            },
        }
        let url = format!(
            "https://api.telegram.org/bot{}/unbanChatMember",
            config.telegram_bot_token
        );
        let ret = client.post(&url).form(&params).send().await.unwrap();
        println!("unban chat member ret {:?}",ret);
        return HttpResponse::Ok().json(ChallengeResponse {
            success: false,
            error: None,
        });
    }

    HttpResponse::Ok().json(ChallengeResponse {
        success: true,
        error: None,
    })
}