use std::time::Duration;

use crate::error::NapcatError;
use serde::{Deserialize, Serialize};

use crate::protocol::{Id, MessageSegment};

#[derive(Clone)]
pub struct NapcatClient {
    client: reqwest::Client,
    base_url: String,
    api_token: String,
}

#[derive(Serialize)]
struct SendGroupMessage<'a> {
    group_id: &'a str,
    message: &'a [MessageSegment],
    auto_escape: bool,
}

#[derive(Deserialize)]
struct ActionResponse {
    status: String,
    retcode: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    wording: String,
}

impl NapcatClient {
    pub fn new(
        base_url: impl Into<String>,
        api_token: impl Into<String>,
        timeout_ms: u64,
    ) -> Result<Self, NapcatError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(NapcatError::Build)?;
        Ok(Self {
            client,
            base_url: base_url.into().trim_end_matches('/').into(),
            api_token: api_token.into(),
        })
    }

    pub async fn send_group_message(
        &self,
        group_id: &Id,
        message: &[MessageSegment],
    ) -> Result<(), NapcatError> {
        let response = self
            .client
            .post(format!("{}/send_group_msg", self.base_url))
            .bearer_auth(&self.api_token)
            .json(&SendGroupMessage {
                group_id: group_id.as_str(),
                message,
                auto_escape: false,
            })
            .send()
            .await
            .map_err(NapcatError::Transport)?;
        if !response.status().is_success() {
            return Err(NapcatError::Http(response.status()));
        }
        let body: ActionResponse = response.json().await.map_err(NapcatError::Transport)?;
        if body.status == "ok" && body.retcode == 0 {
            Ok(())
        } else {
            Err(NapcatError::Rejected {
                retcode: body.retcode,
                message: if body.message.is_empty() {
                    body.wording
                } else {
                    body.message
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, http::HeaderMap, routing::post};
    use serde_json::{Value, json};

    use super::*;

    #[tokio::test]
    #[ignore = "requires loopback networking"]
    async fn sends_the_documented_request_once_with_bearer_auth() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let app = Router::new().route(
            "/send_group_msg",
            post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let handler_calls = handler_calls.clone();
                async move {
                    handler_calls.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(headers.get("authorization").unwrap(), "Bearer api-secret");
                    assert_eq!(body["group_id"], "123");
                    assert_eq!(body["auto_escape"], false);
                    assert_eq!(body["message"][0]["type"], "text");
                    Json(json!({
                        "status": "ok", "retcode": 0, "data": {"message_id": 1},
                        "message": "", "wording": ""
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = NapcatClient::new(format!("http://{address}"), "api-secret", 1_000).unwrap();

        client
            .send_group_message(&Id::new("123").unwrap(), &[MessageSegment::text("hello")])
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
