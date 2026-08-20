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

#[derive(Serialize)]
struct GroupMemberRequest<'a> {
    group_id: &'a str,
    user_id: &'a str,
}

#[derive(Serialize)]
struct SetGroupSpecialTitle<'a> {
    group_id: &'a str,
    user_id: &'a str,
    special_title: &'a str,
}

#[derive(Deserialize)]
struct ActionResponse<T = serde_json::Value> {
    status: String,
    retcode: i64,
    data: T,
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
        action_data(body).map(|_| ())
    }

    pub async fn group_member_role(
        &self,
        group_id: &Id,
        user_id: &Id,
    ) -> Result<String, NapcatError> {
        let body = self
            .post_action(
                "/get_group_member_info",
                &GroupMemberRequest {
                    group_id: group_id.as_str(),
                    user_id: user_id.as_str(),
                },
            )
            .await?;
        let body: ActionResponse = body.json().await.map_err(NapcatError::Transport)?;
        let member = action_data(body)?;
        member
            .get("role")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or(NapcatError::InvalidResponse("group member role is missing"))
    }

    pub async fn set_group_special_title(
        &self,
        group_id: &Id,
        user_id: &Id,
        special_title: &str,
    ) -> Result<(), NapcatError> {
        let body = self
            .post_action(
                "/set_group_special_title",
                &SetGroupSpecialTitle {
                    group_id: group_id.as_str(),
                    user_id: user_id.as_str(),
                    special_title,
                },
            )
            .await?;
        let body: ActionResponse = body.json().await.map_err(NapcatError::Transport)?;
        action_data(body).map(|_| ())
    }

    async fn post_action<T: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<reqwest::Response, NapcatError> {
        let response = self
            .client
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_token)
            .json(body)
            .send()
            .await
            .map_err(NapcatError::Transport)?;
        if !response.status().is_success() {
            return Err(NapcatError::Http(response.status()));
        }
        Ok(response)
    }
}

fn action_data<T>(body: ActionResponse<T>) -> Result<T, NapcatError> {
    if body.status == "ok" && body.retcode == 0 {
        Ok(body.data)
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
    async fn sends_documented_requests_once_with_bearer_auth() {
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let member_calls = calls.clone();
        let title_calls = calls.clone();
        let app = Router::new()
            .route(
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
            )
            .route(
                "/get_group_member_info",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let member_calls = member_calls.clone();
                    async move {
                        member_calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(headers.get("authorization").unwrap(), "Bearer api-secret");
                        assert_eq!(body, json!({"group_id": "123", "user_id": "999"}));
                        Json(json!({
                            "status": "ok", "retcode": 0, "data": {"role": "owner"},
                            "message": "", "wording": ""
                        }))
                    }
                }),
            )
            .route(
                "/set_group_special_title",
                post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let title_calls = title_calls.clone();
                    async move {
                        title_calls.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(headers.get("authorization").unwrap(), "Bearer api-secret");
                        assert_eq!(
                            body,
                            json!({
                                "group_id": "123", "user_id": "456",
                                "special_title": "best member"
                            })
                        );
                        Json(json!({
                            "status": "ok", "retcode": 0, "data": {},
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
        assert_eq!(
            client
                .group_member_role(&Id::new("123").unwrap(), &Id::new("999").unwrap())
                .await
                .unwrap(),
            "owner"
        );
        client
            .set_group_special_title(
                &Id::new("123").unwrap(),
                &Id::new("456").unwrap(),
                "best member",
            )
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        server.abort();
    }
}
