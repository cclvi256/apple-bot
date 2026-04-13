pub mod webhook;

use axum::{
    Router,
    response::Response,
    routing::{get, post},
};

pub fn app_routes() -> Router {
    Router::new()
        .route("/", post(event_handler))
        .route("/ping", get(ping_handler))
        .nest("/webhook", webhook::router())
}

async fn ping_handler() -> &'static str {
    "Pong!"
}

async fn event_handler(_body: String) -> Response<String> {
    Response::new("".into())
}
