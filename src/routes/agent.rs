use std::collections::HashMap;
use actix_web::{get, HttpResponse, post, Responder, web};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use teloxide::{Bot, respond};
use teloxide::prelude::{Message,Requester};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};
use teloxide::payloads::SendMessageSetters;

#[derive(Debug, Serialize)]
pub struct Agent {
    pub agent_name: String,
    pub subject_address: String,
    pub created_at: chrono::NaiveDateTime,
}

#[derive(Debug, Serialize)]
pub struct AgentListResponse {
    pub agents: Vec<Agent>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

#[derive(Debug, Serialize)]
pub struct AgentResponse {
    pub agent: Option<Agent>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentDetailResponse {
    pub agent_name: String,
    pub subject_address: String,
    pub invite_url: String,
    pub bio: Option<String>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddTelegramBotRequest {
    pub bot_token: String,
    pub chat_group_id: String,
    pub subject_address: String,
    pub agent_name: String,
    pub invite_url: String,
    pub bio: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddTelegramBotResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
#[post("/add_tg_bot")]
async fn handle_add_tg_bot(
    data: web::Json<AddTelegramBotRequest>,
    pool: web::Data<PgPool>,
) -> impl Responder {
    let subject_address = data.subject_address.to_lowercase().trim_start_matches("0x").to_owned();
    // Store bot information in database
    let result = sqlx::query!(
        "INSERT INTO telegram_bots (agent_name, bot_token, chat_group_id, subject_address, invite_url, bio) VALUES ($1, $2, $3, $4, $5, $6)",
        data.agent_name,
        data.bot_token,
        data.chat_group_id,
        subject_address.clone(),
        data.invite_url,
        data.bio
    )
        .execute(pool.get_ref())
        .await;

    match result {
        Ok(_) => {
            println!("New Telegram bot added, Agent: {}", data.agent_name);

            // Start new bot processing task
            let bot_token = data.bot_token.clone();
            tokio::spawn(async move {
                let bot = Bot::new(&bot_token);
                println!("Starting new Telegram bot, Token: {}", bot_token);
                teloxide::repl(bot, move |bot: Bot, msg: Message| {
                    let subject = subject_address.clone();
                    async move {
                        if let Some(new_chat_members) = msg.new_chat_members() {
                            for user in new_chat_members {
                                println!(
                                    "[newChatMember] chat ID: {}, user ID: {}, user name: @{}",
                                    msg.chat.id,
                                    user.id,
                                    user.username.as_deref().unwrap_or("nick user")
                                );

                                let url_str = format!("http://38.54.24.5:3000/web3-sign?challenge={}&subject={}", user.id, subject);
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
                    }
                }).await;
            });

            HttpResponse::Ok().json(AddTelegramBotResponse {
                success: true,
                error: None,
            })
        },
        Err(e) => {
            println!("Failed to add Telegram bot: {:?}", e);
            HttpResponse::InternalServerError().json(AddTelegramBotResponse {
                success: false,
                error: Some(format!("Failed to add bot: {}", e)),
            })
        }
    }
}

#[get("/agents")]
async fn get_agents(
    query: web::Query<HashMap<String, String>>,
    pool: web::Data<PgPool>,
) -> impl Responder {
    // Parse pagination parameters
    let page = query.get("page").and_then(|p| p.parse::<i64>().ok()).unwrap_or(1);
    let page_size = query.get("page_size").and_then(|ps| ps.parse::<i64>().ok()).unwrap_or(10);

    if page < 1 || page_size < 1 {
        return HttpResponse::BadRequest().json(serde_json::json!({
            "success": false,
            "error": "Invalid pagination parameters"
        }));
    }

    let offset = (page - 1) * page_size;

    // Get total count
    let total_result = sqlx::query!(
        "SELECT COUNT(*) as count FROM telegram_bots"
    )
        .fetch_one(pool.get_ref())
        .await;

    let total = match total_result {
        Ok(record) => record.count.unwrap_or(0),
        Err(e) => {
            return HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "error": format!("Database error: {}", e)
            }));
        }
    };

    // Get paginated agents
    let agents_result = sqlx::query_as!(
        Agent,
        "SELECT agent_name, subject_address, created_at  FROM telegram_bots ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        page_size,
        offset
    )
        .fetch_all(pool.get_ref())
        .await;

    match agents_result {
        Ok(agents) => {
            HttpResponse::Ok().json(AgentListResponse {
                agents,
                total,
                page,
                page_size,
            })
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(serde_json::json!({
                "success": false,
                "error": format!("Database error: {}", e)
            }))
        }
    }
}

#[get("/agents/{agent_name}")]
async fn get_agent_by_name(
    path: web::Path<String>,
    pool: web::Data<PgPool>,
) -> impl Responder {
    let agent_name = path.into_inner();

    let agent_result = sqlx::query_as!(
        Agent,
        "SELECT agent_name, subject_address, created_at FROM telegram_bots WHERE agent_name = $1",
        agent_name
    )
        .fetch_optional(pool.get_ref())
        .await;

    match agent_result {
        Ok(agent) => {
            HttpResponse::Ok().json(AgentResponse {
                agent,
                success: true,
                error: None,
            })
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(AgentResponse {
                agent: None,
                success: false,
                error: Some(format!("Database error: {}", e)),
            })
        }
    }
}

#[get("/agent/detail/{agent_name}")]
async fn get_agent_detail(
    path: web::Path<String>,
    pool: web::Data<PgPool>,
) -> impl Responder {
    let agent_name = path.into_inner();

    // Query agent details from database
    let agent_result = sqlx::query!(
        "SELECT agent_name, subject_address, invite_url, bio FROM telegram_bots WHERE agent_name = $1",
        agent_name
    )
        .fetch_optional(pool.get_ref())
        .await;

    match agent_result {
        Ok(Some(agent)) => {
            HttpResponse::Ok().json(AgentDetailResponse {
                agent_name: agent.agent_name,
                subject_address: agent.subject_address,
                invite_url: agent.invite_url,
                bio: agent.bio,
                success: true,
                error: None,
            })
        },
        Ok(None) => {
            HttpResponse::NotFound().json(AgentDetailResponse {
                agent_name: String::new(),
                subject_address: String::new(),
                invite_url: String::new(),
                bio: None,
                success: false,
                error: Some("Agent not found".to_string()),
            })
        },
        Err(e) => {
            HttpResponse::InternalServerError().json(AgentDetailResponse {
                agent_name: String::new(),
                subject_address: String::new(),
                invite_url: String::new(),
                bio: None,
                success: false,
                error: Some(format!("Database error: {}", e)),
            })
        }
    }
}