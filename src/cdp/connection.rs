//! Chrome DevTools Protocol client over a raw WebSocket.
//! Mirrors frappe/utils/pdf_generator/cdp_connection.py: id-routed commands plus
//! an event stream that page-level code filters by session/target/frame.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_tungstenite::tungstenite::Message;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsWriter = futures_util::stream::SplitSink<WsStream, Message>;

pub struct CdpClient {
    writer: Arc<Mutex<WsWriter>>,
    next_id: AtomicU64,
    pending: Pending,
    events: broadcast::Sender<Arc<Value>>,
}

impl CdpClient {
    pub async fn connect(url: &str) -> Result<Arc<Self>> {
        let (stream, _resp) = tokio_tungstenite::connect_async(url).await?;
        let (writer, mut reader) = stream.split();
        let (events_tx, _) = broadcast::channel::<Arc<Value>>(8192);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let client = Arc::new(Self {
            writer: Arc::new(Mutex::new(writer)),
            next_id: AtomicU64::new(0),
            pending: pending.clone(),
            events: events_tx.clone(),
        });

        // Reader task: route id-matched responses to waiters, broadcast events.
        tokio::spawn(async move {
            while let Some(msg) = reader.next().await {
                let Ok(msg) = msg else { break };
                let text = match msg {
                    Message::Text(t) => t.to_string(),
                    Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(id) = value.get("id").and_then(|v| v.as_u64()) {
                    if let Some(tx) = pending.lock().await.remove(&id) {
                        let _ = tx.send(value);
                    }
                } else if value.get("method").is_some() {
                    let _ = events_tx.send(Arc::new(value));
                }
            }
        });

        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Value>> {
        self.events.subscribe()
    }

    async fn write(&self, id: u64, method: &str, params: Value, session_id: Option<&str>) -> Result<()> {
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            message["sessionId"] = Value::String(sid.to_string());
        }
        let text = serde_json::to_string(&message)?;
        self.writer.lock().await.send(Message::Text(text.into())).await?;
        Ok(())
    }

    /// Send a command and return its `result` object, erroring on a CDP `error`.
    pub async fn send(&self, method: &str, params: Value, session_id: Option<&str>) -> Result<Value> {
        let response = self.send_raw(method, params, session_id).await?;
        if let Some(err) = response.get("error") {
            bail!("CDP error for {method}: {err}");
        }
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send a command and return the full response envelope (result or error).
    pub async fn send_raw(&self, method: &str, params: Value, session_id: Option<&str>) -> Result<Value> {
        let rx = self.send_nowait(method, params, session_id).await?;
        rx.await.map_err(|_| anyhow!("CDP connection closed before response to {method}"))
    }

    /// Send a command, returning a receiver for its response without awaiting it.
    pub async fn send_nowait(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<oneshot::Receiver<Value>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.write(id, method, params, session_id).await?;
        Ok(rx)
    }
}
