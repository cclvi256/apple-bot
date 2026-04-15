// pub mod message;
// pub mod misc;

use reqwest::{
    Client,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::Value;
use std::{error::Error, fmt};

#[derive(Debug, Clone)]
pub struct NapcatConfig {
    pub base_url: String,
    pub access_token: String,
}

#[derive(Debug, Clone)]
pub struct NapcatClient {
    client: Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct NapcatResponse<T = Value> {
    pub status: String,
    pub retcode: i64,
    pub data: T,
    pub message: Option<String>,
    pub echo: Option<String>,
    pub wording: Option<String>,
}

#[derive(Debug)]
pub enum NapcatApiError {
    InvalidToken(reqwest::header::InvalidHeaderValue),
    Http(reqwest::Error),
}

impl fmt::Display for NapcatApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken(_) => f.write_str("invalid NapCat access token"),
            Self::Http(err) => err.fmt(f),
        }
    }
}

impl Error for NapcatApiError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidToken(err) => Some(err),
            Self::Http(err) => Some(err),
        }
    }
}

impl From<reqwest::Error> for NapcatApiError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

impl NapcatClient {
    pub fn new(config: NapcatConfig) -> Result<Self, NapcatApiError> {
        let mut headers = HeaderMap::new();
        let bearer = format!("Bearer {}", config.access_token);
        let authorization =
            HeaderValue::from_str(&bearer).map_err(NapcatApiError::InvalidToken)?;
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = Client::builder().default_headers(headers).build()?;
        let base_url = config.base_url.trim_end_matches('/').to_string();

        Ok(Self { client, base_url })
    }

    pub async fn request(
        &self,
        path: &str,
        data: Value,
    ) -> Result<NapcatResponse<Value>, NapcatApiError> {
        napcat_request(self, path, data).await
    }
}

pub async fn napcat_request(
    client: &NapcatClient,
    path: &str,
    data: Value,
) -> Result<NapcatResponse<Value>, NapcatApiError> {
    let path = path.trim_start_matches('/');
    let url = format!("{}/{}", client.base_url, path);

    let response = client
        .client
        .post(url)
        .json(&data)
        .send()
        .await?
        .error_for_status()?;

    Ok(response.json().await?)
}
