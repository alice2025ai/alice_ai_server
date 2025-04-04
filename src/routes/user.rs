use crate::db::operations::get_user_shares;
use actix_web::{web, error, HttpResponse, Error, get};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[derive(Serialize)]
pub struct UserSharesResponse {
    user_address: String,
    shares: Vec<SubjectShare>,
}

#[derive(Serialize)]
pub struct SubjectShare {
    subject_address: String,
    shares_amount: String,
}

// API endpoint to get all shares for a user
#[get("/users/{user_address}/shares")]
pub async fn get_user_shares_handler(
    pool: web::Data<PgPool>,
    path: web::Path<String>,
) -> Result<web::Json<UserSharesResponse>, actix_web::Error> {
    let user_address = path.into_inner().to_lowercase().trim_start_matches("0x").to_owned();
    let shares = get_user_shares(&pool, &user_address)
        .await
        .map_err(|_| actix_web::error::ErrorInternalServerError("数据库操作失败"))?;
    
    let subject_shares = shares
        .into_iter()
        .map(|share| SubjectShare {
            subject_address: share.subject,
            shares_amount: share.share_amount.to_string(),
        })
        .collect();
    
    Ok(web::Json(UserSharesResponse {
        user_address,
        shares: subject_shares,
    }))
} 