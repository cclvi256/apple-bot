use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

use crate::{bot::Bot, protocol::Event};

type HmacSha1 = Hmac<Sha1>;

#[derive(Clone)]
pub struct AppState {
    bot: Arc<Bot>,
    webhook_token: Arc<Vec<u8>>,
    max_body_bytes: usize,
}

impl AppState {
    pub fn new(bot: Arc<Bot>, webhook_token: Vec<u8>, max_body_bytes: usize) -> Self {
        Self {
            bot,
            webhook_token: Arc::new(webhook_token),
            max_body_bytes,
        }
    }
}

pub fn router(state: AppState) -> Router {
    let max_body_bytes = state.max_body_bytes;
    Router::new()
        .route("/onebot/v11/events", post(event))
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state)
}

async fn live() -> StatusCode {
    StatusCode::NO_CONTENT
}

async fn ready(State(state): State<AppState>) -> StatusCode {
    if state.bot.ready().await {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn event(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !valid_signature(&headers, &body, &state.webhook_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let event: Event = match serde_json::from_slice(&body) {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "rejected malformed OneBot event");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let self_id_matches = headers
        .get("x-self-id")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == event.self_id.as_str());
    if !self_id_matches {
        return StatusCode::BAD_REQUEST.into_response();
    }

    match state.bot.process(event).await {
        Ok(Some(operation)) => (StatusCode::OK, axum::Json(operation)).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(%error, "failed to process OneBot event");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn valid_signature(headers: &HeaderMap, body: &[u8], token: &[u8]) -> bool {
    let Some(signature) = headers
        .get("x-signature")
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(mut mac) = HmacSha1::new_from_slice(token) else {
        return false;
    };
    mac.update(body);
    let expected = format!("sha1={}", hex::encode(mac.finalize().into_bytes()));
    expected.as_bytes().ct_eq(signature.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::{
        body::Body,
        http::{HeaderValue, Request},
    };
    use tempfile::tempdir;
    use tower::ServiceExt;

    use super::*;
    use crate::{config::Config, store::FeatureStore};

    #[test]
    fn checks_hmac_over_the_exact_body() {
        let body = br#"{"post_type":"message"}"#;
        let mut mac = HmacSha1::new_from_slice(b"secret").unwrap();
        mac.update(body);
        let signature = format!("sha1={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert("x-signature", HeaderValue::from_str(&signature).unwrap());

        assert!(valid_signature(&headers, body, b"secret"));
        assert!(!valid_signature(
            &headers,
            br#"{ "post_type":"message"}"#,
            b"secret"
        ));
    }

    #[tokio::test]
    async fn webhook_rejects_bad_signatures_and_returns_quick_operations() {
        let directory = tempdir().unwrap();
        let url = format!(
            "sqlite://{}?mode=rwc",
            directory.path().join("server.sqlite").display()
        );
        let config = Config::for_test(url.clone());
        let store = FeatureStore::connect(&url).await.unwrap();
        let bot = Arc::new(Bot::new(&config, store, HashMap::new()));
        let app = router(AppState::new(bot, b"secret".to_vec(), 1024 * 1024));
        let body = br#"{
            "post_type":"message", "message_type":"group", "sub_type":"normal",
            "self_id":"999", "message_id":1, "user_id":"10000", "group_id":"200",
            "message":[{"type":"text","data":{"text":".enable dice"}}],
            "sender":{"role":"member"}
        }"#;

        let unauthorized = app
            .clone()
            .oneshot(
                Request::post("/onebot/v11/events")
                    .header("x-self-id", "999")
                    .header("x-signature", "sha1=bad")
                    .body(Body::from(body.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let mut mac = HmacSha1::new_from_slice(b"secret").unwrap();
        mac.update(body);
        let signature = format!("sha1={}", hex::encode(mac.finalize().into_bytes()));
        let response = app
            .oneshot(
                Request::post("/onebot/v11/events")
                    .header("x-self-id", "999")
                    .header("x-signature", signature)
                    .body(Body::from(body.as_slice()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({
                "reply": [{"type":"text", "data":{"text":"Dice statistics enabled."}}],
                "at_sender": false
            })
        );
    }
}
