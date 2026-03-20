# predict_fun_sdk

Blazing fast async Rust SDK client for the Predict.fun REST API.

## Features

- Full REST coverage of documented `dev.predict.fun` endpoints
- Auth support:
  - `x-api-key`
  - JWT bearer token
  - OAuth bearer token
- Strongly typed request/query payloads for common routes
- Generic JSON responses (`serde_json::Value`) for forward compatibility
- `reqwest` + `rustls` + HTTP/2

## Install

```toml
[dependencies]
predict_fun_sdk = { path = "." }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## Quick Start

```rust
use predict_fun_sdk::{AuthRequest, Environment, PredictClient, SearchQuery};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = PredictClient::from_environment(Environment::Testnet)?;

    // Public endpoints
    let categories = client.get_categories(None).await?;
    println!("{}", categories);

    let search = SearchQuery {
        query: "bitcoin".to_string(),
        include_resolved: Some(false),
        limit: Some(10),
    };
    let results = client.search_categories_and_markets(&search).await?;
    println!("{}", results);

    // Auth flow
    let auth_message = client.get_auth_message().await?;
    println!("Sign this message: {}", auth_message["data"]["message"]);

    // After signing externally:
    let jwt = client
        .get_jwt_with_valid_signature(&AuthRequest {
            signer: "0x...".into(),
            signature: "0x...".into(),
            message: "signed message".into(),
        })
        .await?;

    let token = jwt["data"]["token"].as_str().unwrap_or_default().to_string();
    client.set_jwt_token(token)?;

    // Authenticated endpoint
    let me = client.get_connected_account().await?;
    println!("{}", me);

    Ok(())
}
```

## Covered API Functions

- Authorization: `get_auth_message`, `get_jwt_with_valid_signature`
- Categories: `get_categories`, `get_category_by_slug`, `get_all_tags`
- Markets: `get_markets`, `get_market_by_id`, `get_market_statistics`, `get_market_last_sale_information`, `get_market_orderbook`
- Orders: `get_order_by_hash`, `get_orders`, `get_order_match_events`, `create_order`, `remove_orders_from_orderbook`
- Accounts: `get_connected_account`, `get_account_activity`, `set_referral`
- Positions: `get_positions`, `get_positions_by_address`
- Search: `search_categories_and_markets`
- OAuth (restricted): `oauth_finalize_connection`, `oauth_get_orders`, `oauth_create_order`, `oauth_cancel_orders`, `oauth_get_positions`

## WebSocket Support

Implements Predict websocket protocol at `wss://ws.predict.fun/ws`:

- Typed request/response envelopes (`subscribe`, `unsubscribe`, `heartbeat`)
- Topic builders:
  - `predictOrderbook/{marketId}`
  - `assetPriceUpdate/{priceFeedId}`
  - `predictWalletEvents/{jwt}`
- Automatic heartbeat response
- Automatic reconnect + auto-resubscribe to active topics

```rust
use predict_fun_sdk::{
    PredictClient, Environment, PredictWebSocketConfig, WsServerMessage, WsTopic
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = PredictClient::from_environment(Environment::Mainnet)?;
    client.set_api_key("your-api-key")?;

    let ws = client
        .connect_websocket(PredictWebSocketConfig::default())
        .await?;

    ws.subscribe(WsTopic::PredictOrderbook {
        market_id: "12345".to_string(),
    })?;

    let mut events = ws.subscribe_events();
    while let Ok(msg) = events.recv().await {
        match msg {
            WsServerMessage::Push { topic, data } => {
                println!("{} {}", topic, data);
            }
            WsServerMessage::Response {
                request_id,
                success,
                error,
                ..
            } => {
                println!("req={} success={} error={:?}", request_id, success, error);
            }
        }
    }

    Ok(())
}
```
