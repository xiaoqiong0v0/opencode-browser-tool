//! 原始 CDP WebSocket 客户端
//! 设计:connect 时建立读写循环,外部通过 mpsc channel 发送命令
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// 事件监听器
pub type EventListener = Box<dyn Fn(Value) + Send + Sync>;

/// 事件分发器(会话事件广播)
pub trait EventDispatcher: Send + Sync {
    fn dispatch(&self, method: &str, params: Value);
}

/// CDP 客户端(可 Clone,内部共享状态)
#[derive(Clone)]
pub struct CdpClient {
    inner: Arc<Mutex<CdpInner>>,
    /// 命令发送通道
    tx: mpsc::Sender<String>,
    /// 全局事件分发器
    dispatcher: Arc<std::sync::Mutex<Option<Arc<dyn EventDispatcher>>>>,
}

struct CdpInner {
    /// 待响应请求(id -> 回调)
    pending: HashMap<u64, oneshot::Sender<Result<Value, String>>>,
    /// 事件监听(method -> 监听器)
    listeners: HashMap<String, Vec<EventListener>>,
    next_id: u64,
    connected: bool,
}

impl CdpClient {
    pub fn new() -> Self {
        let (tx, _rx) = mpsc::channel(64);
        Self {
            inner: Arc::new(Mutex::new(CdpInner {
                pending: HashMap::new(),
                listeners: HashMap::new(),
                next_id: 1,
                connected: false,
            })),
            tx,
            dispatcher: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// 设置全局事件分发器(如 EventHub)
    pub fn set_dispatcher(&mut self, dispatcher: Arc<dyn EventDispatcher>) {
        *self.dispatcher.lock().unwrap() = Some(dispatcher);
    }

    /// 连接调试 WebSocket 地址,启动读写循环
    pub async fn connect(&mut self, ws_url: &str) -> Result<(), String> {
        let (ws, _) = connect_async(ws_url)
            .await
            .map_err(|e| format!("CDP connect failed: {e}"))?;
        let (mut write, mut read) = ws.split();
        let (tx, mut rx) = mpsc::channel::<String>(64);

        {
            let mut inner = self.inner.lock().await;
            inner.connected = true;
            self.tx = tx.clone();
        }

        let client = self.clone();
        // 写循环:转发命令到 WebSocket
        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                if let Err(e) = write.send(Message::Text(text.into())).await {
                    eprintln!("[cdp] write error: {e}");
                    break;
                }
            }
            let _ = write.close().await;
        });

        // 读循环:分发响应和事件
        let client2 = self.clone();
        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(data) = serde_json::from_str::<Value>(&text) {
                            client2.dispatch(data).await;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
            client2.mark_closed().await;
        });

        Ok(())
    }

    /// 分发 CDP 消息:有 id 是响应,无 id 是事件
    async fn dispatch(&self, data: Value) {
        if let Some(id) = data.get("id").and_then(|v| v.as_u64()) {
            let sender = {
                let mut inner = self.inner.lock().await;
                inner.pending.remove(&id)
            };
            if let Some(tx) = sender {
                if let Some(err) = data.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("CDP error")
                        .to_string();
                    let _ = tx.send(Err(msg));
                } else {
                    let result = data.get("result").cloned().unwrap_or(Value::Null);
                    let _ = tx.send(Ok(result));
                }
            }
        } else if let Some(method) = data.get("method").and_then(|m| m.as_str()) {
            let params = data.get("params").cloned().unwrap_or(Value::Null);
            // 全局事件分发(console/network 等)
            if let Some(d) = self.dispatcher.lock().unwrap().as_ref() {
                d.dispatch(method, params.clone());
            }
            // 取出监听器(短暂取出,避免持锁调用回调)
            let mut inner = self.inner.lock().await;
            let listeners = inner.listeners.remove(method);
            drop(inner);
            if let Some(listeners) = listeners {
                for l in &listeners {
                    l(params.clone());
                }
                // 放回监听器(先移除后放回,避免持锁调用回调)
                let mut inner = self.inner.lock().await;
                inner.listeners.insert(method.to_string(), listeners);
            }
        }
    }

    /// 发送 CDP 命令并等待结果
    pub async fn send(&self, method: &str, params: Value) -> Result<Value, String> {
        let (id, tx) = {
            let mut inner = self.inner.lock().await;
            if !inner.connected {
                return Err("CDP not connected".into());
            }
            let id = inner.next_id;
            inner.next_id += 1;
            let (tx, rx) = oneshot::channel();
            inner.pending.insert(id, tx);
            (id, rx)
        };
        let msg = serde_json::json!({ "id": id, "method": method, "params": params });
        self.tx
            .send(msg.to_string())
            .await
            .map_err(|_| "CDP send channel closed".to_string())?;
        match tx.await {
            Ok(r) => r,
            Err(_) => Err("CDP response dropped".into()),
        }
    }

    /// 订阅事件,返回取消订阅的闭包
    pub fn on(&self, method: &str, listener: EventListener) -> impl Fn() {
        let mut inner = self.inner.blocking_lock();
        inner
            .listeners
            .entry(method.to_string())
            .or_default()
            .push(listener);
        // 返回一个无操作的取消闭包(简化)
        || {}
    }

    /// 标记连接关闭,唤醒所有待响应
    async fn mark_closed(&self) {
        let mut inner = self.inner.lock().await;
        inner.connected = false;
        let pending = std::mem::take(&mut inner.pending);
        for (_, tx) in pending {
            let _ = tx.send(Err("CDP connection closed".into()));
        }
    }

    /// 是否已连接
    pub async fn is_connected(&self) -> bool {
        self.inner.lock().await.connected
    }
}
