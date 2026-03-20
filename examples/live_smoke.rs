use predict_fun_sdk::{Environment, PredictClient, PredictClientConfig, SearchQuery};
use serde_json::Value;
use std::env;

fn summarize_json_keys(v: &Value) -> String {
    match v {
        Value::Object(map) => map.keys().cloned().collect::<Vec<_>>().join(", "),
        _ => "non-object response".to_string(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = env::var("PREDICT_API_KEY").ok();
    let base_url = env::var("PREDICT_BASE_URL").unwrap_or_else(|_| Environment::Mainnet.base_url().to_string());

    let client = PredictClient::new(PredictClientConfig {
        base_url,
        api_key,
        ..Default::default()
    })?;

    let auth_message = client.get_auth_message().await?;
    println!("get_auth_message: ok, keys=[{}]", summarize_json_keys(&auth_message));

    let markets = client.get_markets(None).await?;
    println!("get_markets: ok, keys=[{}]", summarize_json_keys(&markets));

    let search = client
        .search_categories_and_markets(&SearchQuery {
            query: "bitcoin".to_string(),
            include_resolved: Some(false),
            limit: Some(3),
        })
        .await?;
    println!("search_categories_and_markets: ok, keys=[{}]", summarize_json_keys(&search));

    Ok(())
}
