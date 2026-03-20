use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Display;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use thiserror::Error;

pub mod websocket;
pub use websocket::{PREDICT_WS_ENDPOINT, PredictWebSocket, PredictWebSocketConfig, WsRequest, WsServerMessage, WsTopic};

pub const MAINNET_BASE_URL: &str = "https://api.predict.fun";
pub const TESTNET_BASE_URL: &str = "https://api-testnet.predict.fun";

#[derive(Debug, Clone, Copy)]
pub enum Environment {
    Mainnet,
    Testnet,
}

impl Environment {
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Mainnet => MAINNET_BASE_URL,
            Self::Testnet => TESTNET_BASE_URL,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PredictClientConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub jwt_token: Option<String>,
    pub oauth_token: Option<String>,
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for PredictClientConfig {
    fn default() -> Self {
        Self {
            base_url: TESTNET_BASE_URL.to_string(),
            api_key: None,
            jwt_token: None,
            oauth_token: None,
            timeout: Duration::from_secs(10),
            user_agent: format!("predict-fun-sdk-rs/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

#[derive(Debug, Error)]
pub enum PredictError {
    #[error("invalid configuration: {0}")]
    Config(&'static str),
    #[error("failed to build HTTP client: {0}")]
    ClientBuild(reqwest::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api returned status {status}: {body}")]
    Api { status: StatusCode, body: String },
    #[error("missing auth token: {0}")]
    MissingAuth(&'static str),
    #[error("lock poisoned: {0}")]
    LockPoisoned(&'static str),
}

pub type Result<T> = std::result::Result<T, PredictError>;

#[derive(Clone)]
pub struct PredictClient {
    http: Client,
    base_url: String,
    api_key: Arc<RwLock<Option<String>>>,
    jwt_token: Arc<RwLock<Option<String>>>,
    oauth_token: Arc<RwLock<Option<String>>>,
}

#[derive(Debug, Clone, Copy)]
enum AuthKind {
    None,
    Jwt,
    Oauth,
}

impl PredictClient {
    pub fn new(config: PredictClientConfig) -> Result<Self> {
        let mut default_headers = HeaderMap::new();
        default_headers.insert("accept", HeaderValue::from_static("application/json"));

        let user_agent =
            HeaderValue::from_str(&config.user_agent).map_err(|_| PredictError::Config("invalid user_agent"))?;
        default_headers.insert("user-agent", user_agent);

        let http = Client::builder()
            .default_headers(default_headers)
            .pool_idle_timeout(Duration::from_secs(90))
            .pool_max_idle_per_host(32)
            .tcp_nodelay(true)
            .timeout(config.timeout)
            .build()
            .map_err(PredictError::ClientBuild)?;

        Ok(Self {
            http,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: Arc::new(RwLock::new(config.api_key)),
            jwt_token: Arc::new(RwLock::new(config.jwt_token)),
            oauth_token: Arc::new(RwLock::new(config.oauth_token)),
        })
    }

    pub fn from_environment(env: Environment) -> Result<Self> {
        Self::new(PredictClientConfig {
            base_url: env.base_url().to_string(),
            ..Default::default()
        })
    }

    pub fn set_api_key(&self, api_key: impl Into<String>) -> Result<()> {
        let mut guard = self
            .api_key
            .write()
            .map_err(|_| PredictError::LockPoisoned("api_key"))?;
        *guard = Some(api_key.into());
        Ok(())
    }

    pub fn set_jwt_token(&self, token: impl Into<String>) -> Result<()> {
        let mut guard = self
            .jwt_token
            .write()
            .map_err(|_| PredictError::LockPoisoned("jwt_token"))?;
        *guard = Some(token.into());
        Ok(())
    }

    pub fn set_oauth_token(&self, token: impl Into<String>) -> Result<()> {
        let mut guard = self
            .oauth_token
            .write()
            .map_err(|_| PredictError::LockPoisoned("oauth_token"))?;
        *guard = Some(token.into());
        Ok(())
    }

    pub async fn connect_websocket(&self, mut config: PredictWebSocketConfig) -> Result<PredictWebSocket> {
        if config.api_key.is_none() {
            config.api_key = self
                .api_key
                .read()
                .map_err(|_| PredictError::LockPoisoned("api_key"))?
                .clone();
        }
        PredictWebSocket::connect(config).await
    }

    pub async fn get_auth_message(&self) -> Result<Value> {
        self.request_json::<(), (), Value>(Method::GET, "/v1/auth/message", None, None, AuthKind::None)
            .await
    }

    pub async fn get_jwt_with_valid_signature(&self, request: &AuthRequest) -> Result<Value> {
        self.request_json(Method::POST, "/v1/auth", None::<&()>, Some(request), AuthKind::None)
            .await
    }

    pub async fn get_categories(&self, query: Option<&GetCategoriesQuery>) -> Result<Value> {
        self.request_json(Method::GET, "/v1/categories", query, None::<&()>, AuthKind::None)
            .await
    }

    pub async fn get_category_by_slug(&self, slug: &str) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/categories/{slug}"),
            None,
            None,
            AuthKind::None,
        )
        .await
    }

    pub async fn get_all_tags(&self) -> Result<Value> {
        self.request_json::<(), (), Value>(Method::GET, "/v1/tags", None, None, AuthKind::None)
            .await
    }

    pub async fn get_markets(&self, query: Option<&GetMarketsQuery>) -> Result<Value> {
        self.request_json(Method::GET, "/v1/markets", query, None::<&()>, AuthKind::None)
            .await
    }

    pub async fn get_market_by_id(&self, market_id: impl Display) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/markets/{market_id}"),
            None,
            None,
            AuthKind::None,
        )
        .await
    }

    pub async fn get_market_statistics(&self, market_id: impl Display) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/markets/{market_id}/stats"),
            None,
            None,
            AuthKind::None,
        )
        .await
    }

    pub async fn get_market_last_sale_information(&self, market_id: impl Display) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/markets/{market_id}/last-sale"),
            None,
            None,
            AuthKind::None,
        )
        .await
    }

    pub async fn get_market_orderbook(&self, market_id: impl Display) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/markets/{market_id}/orderbook"),
            None,
            None,
            AuthKind::None,
        )
        .await
    }

    pub async fn get_order_by_hash(&self, hash: &str) -> Result<Value> {
        self.request_json::<(), (), Value>(
            Method::GET,
            &format!("/v1/orders/{hash}"),
            None,
            None,
            AuthKind::Jwt,
        )
        .await
    }

    pub async fn get_orders(&self, query: Option<&GetOrdersQuery>) -> Result<Value> {
        self.request_json(Method::GET, "/v1/orders", query, None::<&()>, AuthKind::Jwt)
            .await
    }

    pub async fn get_order_match_events(&self, query: Option<&GetOrderMatchEventsQuery>) -> Result<Value> {
        self.request_json(
            Method::GET,
            "/v1/orders/matches",
            query,
            None::<&()>,
            AuthKind::None,
        )
        .await
    }

    pub async fn create_order(&self, request: &CreateOrderRequest) -> Result<Value> {
        self.request_json(Method::POST, "/v1/orders", None::<&()>, Some(request), AuthKind::Jwt)
            .await
    }

    pub async fn remove_orders_from_orderbook(&self, request: &RemoveOrdersRequest) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/orders/remove",
            None::<&()>,
            Some(request),
            AuthKind::Jwt,
        )
        .await
    }

    pub async fn get_connected_account(&self) -> Result<Value> {
        self.request_json::<(), (), Value>(Method::GET, "/v1/account", None, None, AuthKind::Jwt)
            .await
    }

    pub async fn get_account_activity(&self, query: Option<&AccountActivityQuery>) -> Result<Value> {
        self.request_json(
            Method::GET,
            "/v1/account/activity",
            query,
            None::<&()>,
            AuthKind::Jwt,
        )
        .await
    }

    pub async fn set_referral(&self, request: &SetReferralRequest) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/account/referral",
            None::<&()>,
            Some(request),
            AuthKind::Jwt,
        )
        .await
    }

    pub async fn get_positions(&self, query: Option<&PaginationQuery>) -> Result<Value> {
        self.request_json(Method::GET, "/v1/positions", query, None::<&()>, AuthKind::Jwt)
            .await
    }

    pub async fn get_positions_by_address(
        &self,
        address: &str,
        query: Option<&PaginationQuery>,
    ) -> Result<Value> {
        self.request_json(
            Method::GET,
            &format!("/v1/positions/{address}"),
            query,
            None::<&()>,
            AuthKind::None,
        )
        .await
    }

    pub async fn search_categories_and_markets(&self, query: &SearchQuery) -> Result<Value> {
        self.request_json(Method::GET, "/v1/search", Some(query), None::<&()>, AuthKind::None)
            .await
    }

    pub async fn oauth_finalize_connection(&self, request: &OauthSignedRequest<FinalizeOauthData>) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/oauth/finalize",
            None::<&()>,
            Some(request),
            AuthKind::None,
        )
        .await
    }

    pub async fn oauth_get_orders(&self, request: &OauthSignedRequest<OauthGetOrdersData>) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/oauth/orders",
            None::<&()>,
            Some(request),
            AuthKind::Oauth,
        )
        .await
    }

    pub async fn oauth_create_order(&self, request: &OauthSignedRequest<CreateOrderData>) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/oauth/orders/create",
            None::<&()>,
            Some(request),
            AuthKind::Oauth,
        )
        .await
    }

    pub async fn oauth_cancel_orders(&self, request: &OauthSignedRequest<OauthCancelOrdersData>) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/oauth/orders/cancel",
            None::<&()>,
            Some(request),
            AuthKind::Oauth,
        )
        .await
    }

    pub async fn oauth_get_positions(&self, request: &OauthSignedRequest<OauthGetPositionsData>) -> Result<Value> {
        self.request_json(
            Method::POST,
            "/v1/oauth/positions",
            None::<&()>,
            Some(request),
            AuthKind::Oauth,
        )
        .await
    }

    async fn request_json<Q, B, T>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Q>,
        body: Option<&B>,
        auth: AuthKind,
    ) -> Result<T>
    where
        Q: Serialize + ?Sized,
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.http.request(method, &url);

        if let Some(query) = query {
            request = request.query(query);
        }

        if let Some(body) = body {
            request = request.json(body);
        }

        if let Some(api_key) = self
            .api_key
            .read()
            .map_err(|_| PredictError::LockPoisoned("api_key"))?
            .clone()
        {
            request = request.header("x-api-key", api_key);
        }

        request = match auth {
            AuthKind::None => request,
            AuthKind::Jwt => {
                let token = self
                    .jwt_token
                    .read()
                    .map_err(|_| PredictError::LockPoisoned("jwt_token"))?
                    .clone()
                    .ok_or(PredictError::MissingAuth("jwt_token"))?;
                request.bearer_auth(token)
            }
            AuthKind::Oauth => {
                let token = self
                    .oauth_token
                    .read()
                    .map_err(|_| PredictError::LockPoisoned("oauth_token"))?
                    .clone()
                    .ok_or(PredictError::MissingAuth("oauth_token"))?;
                request.bearer_auth(token)
            }
        };

        let response = request.send().await?;
        let status = response.status();

        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(PredictError::Api { status, body });
        }

        Ok(response.json::<T>().await?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEnvelope<T> {
    pub success: bool,
    #[serde(flatten)]
    pub payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub signer: String,
    pub signature: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthSignedRequest<T> {
    pub signer: String,
    pub account: String,
    pub signature: String,
    pub data: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrderRequest {
    pub data: CreateOrderData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrderData {
    pub price_per_share: String,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slippage_bps: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_fill_or_kill: Option<bool>,
    pub order: SignedOrder,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrder {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    pub salt: String,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub expiration: StringOrInt,
    pub nonce: String,
    pub fee_rate_bps: String,
    pub side: u8,
    pub signature_type: u8,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrInt {
    String(String),
    Int(i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveOrdersRequest {
    pub data: IdsData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdsData {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetReferralRequest {
    pub data: ReferralData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferralData {
    pub referral_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalizeOauthData {
    pub timestamp: i64,
    pub client_id: String,
    pub auth_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OauthGetOrdersData {
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OauthGetPositionsData {
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthCancelOrdersData {
    pub timestamp: i64,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaginationQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetCategoriesQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_variant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetMarketsQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_boosted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetOrdersQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GetOrderMatchEventsQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_value_usdt_wei: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_signer_maker: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountActivityQuery {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_resolved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_camel_case_query_fields() {
        let q = SearchQuery {
            query: "btc".into(),
            include_resolved: Some(false),
            limit: Some(10),
        };
        let encoded = serde_urlencoded::to_string(q).expect("query serialization should work");
        assert!(encoded.contains("query=btc"));
        assert!(encoded.contains("includeResolved=false"));
        assert!(encoded.contains("limit=10"));
    }

    #[tokio::test]
    async fn missing_jwt_fails_fast() {
        let client = PredictClient::from_environment(Environment::Testnet).expect("client config is valid");
        let err = client.get_connected_account().await.expect_err("should require JWT token");
        assert!(matches!(err, PredictError::MissingAuth("jwt_token")));
    }
}
