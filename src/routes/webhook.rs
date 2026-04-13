use axum::{Router, routing::post};

pub fn router() -> Router {
    Router::new().route("/", post(webhook_handler))
}

pub async fn webhook_handler(_body: String) -> &'static str {
    "Webhook received"
}