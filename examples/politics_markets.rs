use anyhow::Context;
use predict_fun_sdk::{Environment, PredictClient, PredictClientConfig, SearchQuery};
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = env::var("PREDICT_API_KEY").context("missing PREDICT_API_KEY")?;
    let base_url = env::var("PREDICT_BASE_URL").unwrap_or_else(|_| Environment::Mainnet.base_url().to_string());

    let client = PredictClient::new(PredictClientConfig {
        base_url,
        api_key: Some(api_key),
        ..Default::default()
    })?;

    let response = client
        .search_categories_and_markets(&SearchQuery {
            query: "politics".to_string(),
            include_resolved: Some(false),
            limit: Some(10),
        })
        .await?;

    println!("query=politics");

    if let Some(categories) = response.get("data").and_then(|d| d.get("categories")).and_then(|c| c.as_array()) {
        println!("categories: {}", categories.len());
        for category in categories.iter().take(5) {
            let slug = category.get("slug").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            let title = category.get("title").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            println!("- category: {} ({})", title, slug);
        }
    }

    if let Some(markets) = response.get("data").and_then(|d| d.get("markets")).and_then(|m| m.as_array()) {
        println!("markets: {}", markets.len());
        for market in markets.iter().take(10) {
            let id = market.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            let title = market.get("title").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            let status = market.get("tradingStatus").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            println!("- market {}: {} [{}]", id, title, status);
        }
    }

    Ok(())
}
