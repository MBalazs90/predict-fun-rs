use anyhow::{Context, bail};
use predict_fun_sdk::{
    Environment, GetMarketsQuery, PredictClient, PredictClientConfig, PredictWebSocketConfig, WsServerMessage, WsTopic,
};
use std::env;
use tokio::time::{Duration, Instant};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api_key = env::var("PREDICT_API_KEY").context("missing PREDICT_API_KEY")?;
    let base_url = env::var("PREDICT_BASE_URL").unwrap_or_else(|_| Environment::Mainnet.base_url().to_string());

    let client = PredictClient::new(PredictClientConfig {
        base_url,
        api_key: Some(api_key.clone()),
        ..Default::default()
    })?;

    let markets = client
        .get_markets(Some(&GetMarketsQuery {
            first: Some(1),
            ..Default::default()
        }))
        .await
        .context("failed to fetch a market id for WS subscription")?;

    let market_id = markets
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| m.get("id"))
        .and_then(|id| id.as_i64())
        .context("could not find market id in get_markets response")?;

    let ws = client
        .connect_websocket(PredictWebSocketConfig {
            api_key: Some(api_key),
            ..Default::default()
        })
        .await
        .context("failed to connect websocket")?;

    let subscribe_id = ws
        .subscribe(WsTopic::PredictOrderbook {
            market_id: market_id.to_string(),
        })
        .context("failed to send subscribe command")?;

    let mut events = ws.subscribe_events();
    let deadline = Instant::now() + Duration::from_secs(35);
    let mut got_subscribe_ack = false;
    let mut got_push = false;
    let mut got_heartbeat = false;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, events.recv()).await {
            Ok(Ok(msg)) => match msg {
                WsServerMessage::Response {
                    request_id,
                    success,
                    error,
                    ..
                } if request_id == subscribe_id => {
                    if success {
                        got_subscribe_ack = true;
                    } else {
                        bail!("subscribe was rejected: {:?}", error);
                    }
                }
                WsServerMessage::Push { topic, .. } => {
                    got_push = true;
                    if topic == "heartbeat" {
                        got_heartbeat = true;
                    }
                }
                _ => {}
            },
            Ok(Err(_)) => continue,
            Err(_) => break,
        }

        if got_subscribe_ack && got_push {
            break;
        }
    }

    ws.close().ok();

    if !got_subscribe_ack {
        bail!("did not receive subscribe ack response from websocket");
    }
    if !got_push {
        bail!("did not receive any push message after subscribing");
    }

    println!("websocket smoke: ok");
    println!("market_id: {}", market_id);
    println!("subscribe_ack: {}", got_subscribe_ack);
    println!("got_push: {}", got_push);
    println!("got_heartbeat: {}", got_heartbeat);

    Ok(())
}
