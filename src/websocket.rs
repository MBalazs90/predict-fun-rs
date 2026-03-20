use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::HeaderName;

use crate::{PredictError, Result};

pub const PREDICT_WS_ENDPOINT: &str = "wss://ws.predict.fun/ws";

#[derive(Debug, Clone)]
pub struct PredictWebSocketConfig {
    pub endpoint: String,
    pub api_key: Option<String>,
    pub max_reconnect_attempts: u32,
    pub max_reconnect_delay: Duration,
}

impl Default for PredictWebSocketConfig {
    fn default() -> Self {
        Self {
            endpoint: PREDICT_WS_ENDPOINT.to_string(),
            api_key: None,
            max_reconnect_attempts: 25,
            max_reconnect_delay: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WsMethod {
    Subscribe,
    Unsubscribe,
    Heartbeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WsRequest {
    pub method: WsMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsErrorPayload {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsServerMessage {
    #[serde(rename = "R", rename_all = "camelCase")]
    Response {
        request_id: u64,
        success: bool,
        #[serde(default)]
        data: Option<Value>,
        #[serde(default)]
        error: Option<WsErrorPayload>,
    },
    #[serde(rename = "M")]
    Push { topic: String, data: Value },
}

#[derive(Debug, Clone)]
pub enum WsTopic {
    PredictOrderbook { market_id: String },
    AssetPriceUpdate { price_feed_id: String },
    PredictWalletEvents { jwt: String },
    Custom(String),
}

impl WsTopic {
    pub fn as_topic_string(&self) -> String {
        match self {
            Self::PredictOrderbook { market_id } => format!("predictOrderbook/{market_id}"),
            Self::AssetPriceUpdate { price_feed_id } => format!("assetPriceUpdate/{price_feed_id}"),
            Self::PredictWalletEvents { jwt } => format!("predictWalletEvents/{jwt}"),
            Self::Custom(raw) => raw.clone(),
        }
    }
}

enum WsCommand {
    Subscribe { topic: String, request_id: u64 },
    Unsubscribe { topic: String, request_id: u64 },
    Raw(WsRequest),
    Close,
}

#[derive(Clone)]
pub struct PredictWebSocket {
    command_tx: mpsc::UnboundedSender<WsCommand>,
    events_tx: broadcast::Sender<WsServerMessage>,
    next_request_id: Arc<AtomicU64>,
}

impl PredictWebSocket {
    pub async fn connect(config: PredictWebSocketConfig) -> Result<Self> {
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let (events_tx, _) = broadcast::channel(2048);
        let next_request_id = Arc::new(AtomicU64::new(0));

        let runtime = WsRuntime {
            config,
            command_rx,
            events_tx: events_tx.clone(),
            next_request_id: next_request_id.clone(),
            subscriptions: HashSet::new(),
        };

        tokio::spawn(runtime.run());

        Ok(Self {
            command_tx,
            events_tx,
            next_request_id,
        })
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<WsServerMessage> {
        self.events_tx.subscribe()
    }

    pub fn next_request_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn subscribe(&self, topic: WsTopic) -> Result<u64> {
        let request_id = self.next_request_id();
        self.command_tx
            .send(WsCommand::Subscribe {
                topic: topic.as_topic_string(),
                request_id,
            })
            .map_err(|_| PredictError::Config("websocket command channel is closed"))?;
        Ok(request_id)
    }

    pub fn unsubscribe(&self, topic: WsTopic) -> Result<u64> {
        let request_id = self.next_request_id();
        self.command_tx
            .send(WsCommand::Unsubscribe {
                topic: topic.as_topic_string(),
                request_id,
            })
            .map_err(|_| PredictError::Config("websocket command channel is closed"))?;
        Ok(request_id)
    }

    pub fn send_raw(&self, request: WsRequest) -> Result<()> {
        self.command_tx
            .send(WsCommand::Raw(request))
            .map_err(|_| PredictError::Config("websocket command channel is closed"))?;
        Ok(())
    }

    pub fn close(&self) -> Result<()> {
        self.command_tx
            .send(WsCommand::Close)
            .map_err(|_| PredictError::Config("websocket command channel is closed"))?;
        Ok(())
    }
}

struct WsRuntime {
    config: PredictWebSocketConfig,
    command_rx: mpsc::UnboundedReceiver<WsCommand>,
    events_tx: broadcast::Sender<WsServerMessage>,
    next_request_id: Arc<AtomicU64>,
    subscriptions: HashSet<String>,
}

impl WsRuntime {
    async fn run(mut self) {
        let mut attempt: u32 = 0;

        loop {
            if attempt > self.config.max_reconnect_attempts {
                break;
            }

            let request = match self.build_request() {
                Ok(req) => req,
                Err(_) => break,
            };

            match connect_async(request).await {
                Ok((stream, _)) => {
                    attempt = 0;
                    let (mut writer, mut reader) = stream.split();

                    for topic in &self.subscriptions {
                        let req_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
                        let subscribe_req = WsRequest {
                            method: WsMethod::Subscribe,
                            request_id: Some(req_id),
                            params: Some(vec![topic.clone()]),
                            data: None,
                        };

                        if let Ok(text) = serde_json::to_string(&subscribe_req) {
                            if writer.send(Message::Text(text)).await.is_err() {
                                break;
                            }
                        }
                    }

                    loop {
                        tokio::select! {
                            maybe_cmd = self.command_rx.recv() => {
                                match maybe_cmd {
                                    Some(WsCommand::Subscribe { topic, request_id }) => {
                                        self.subscriptions.insert(topic.clone());
                                        let req = WsRequest {
                                            method: WsMethod::Subscribe,
                                            request_id: Some(request_id),
                                            params: Some(vec![topic]),
                                            data: None,
                                        };
                                        if self.send_request(&mut writer, req).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(WsCommand::Unsubscribe { topic, request_id }) => {
                                        self.subscriptions.remove(&topic);
                                        let req = WsRequest {
                                            method: WsMethod::Unsubscribe,
                                            request_id: Some(request_id),
                                            params: Some(vec![topic]),
                                            data: None,
                                        };
                                        if self.send_request(&mut writer, req).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(WsCommand::Raw(req)) => {
                                        if self.send_request(&mut writer, req).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(WsCommand::Close) | None => {
                                        let _ = writer.send(Message::Close(None)).await;
                                        return;
                                    }
                                }
                            }
                            msg = reader.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(parsed) = serde_json::from_str::<WsServerMessage>(&text) {
                                            if let WsServerMessage::Push { topic, data } = &parsed {
                                                if topic == "heartbeat" {
                                                    let heartbeat = WsRequest {
                                                        method: WsMethod::Heartbeat,
                                                        request_id: None,
                                                        params: None,
                                                        data: Some(data.clone()),
                                                    };
                                                    let _ = self.send_request(&mut writer, heartbeat).await;
                                                }
                                            }
                                            let _ = self.events_tx.send(parsed);
                                        }
                                    }
                                    Some(Ok(Message::Ping(payload))) => {
                                        if writer.send(Message::Pong(payload)).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) => {
                                        break;
                                    }
                                    Some(Err(_)) | None => {
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(_) => {
                    attempt = attempt.saturating_add(1);
                    let exp = attempt.min(10);
                    let delay_ms = (1u64 << exp) * 250;
                    tokio::time::sleep(self.config.max_reconnect_delay.min(Duration::from_millis(delay_ms))).await;
                }
            }
        }
    }

    async fn send_request<S>(&self, writer: &mut S, req: WsRequest) -> std::result::Result<(), ()>
    where
        S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    {
        let text = serde_json::to_string(&req).map_err(|_| ())?;
        writer.send(Message::Text(text)).await.map_err(|_| ())
    }

    fn build_request(&self) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        let mut request = self
            .config
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| PredictError::Config("invalid websocket endpoint"))?;

        if let Some(api_key) = &self.config.api_key {
            request.headers_mut().insert(
                HeaderName::from_static("x-api-key"),
                HeaderValue::from_str(api_key).map_err(|_| PredictError::Config("invalid api key header"))?,
            );
        }

        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_builder_formats_expected_paths() {
        let t = WsTopic::PredictOrderbook { market_id: "123".into() };
        assert_eq!(t.as_topic_string(), "predictOrderbook/123");

        let t = WsTopic::AssetPriceUpdate {
            price_feed_id: "abc".into(),
        };
        assert_eq!(t.as_topic_string(), "assetPriceUpdate/abc");
    }
}
