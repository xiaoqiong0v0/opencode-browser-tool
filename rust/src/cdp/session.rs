//! 会话管理:标签页、控制台日志、网络响应监听
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::client::CdpClient;

/// 控制台日志条目
#[derive(Debug, Clone)]
pub struct ConsoleEntry {
    pub r#type: String,
    pub text: String,
}

/// 待匹配的网络响应
#[derive(Debug, Clone)]
pub struct PendingResponse {
    pub pattern: String,
    pub matched: bool,
    pub result: Option<Value>,
}

/// 会话状态(浏览器级共享)
pub struct SessionState {
    pub console_logs: Arc<Mutex<Vec<ConsoleEntry>>>,
    pub pending_responses: Arc<Mutex<Vec<PendingResponse>>>,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            console_logs: Arc::new(Mutex::new(Vec::new())),
            pending_responses: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 订阅控制台事件(需在连接页面 target 后调用)
    pub fn attach(&self, client: &CdpClient) {
        // Runtime.enable 后才能收到 consoleAPICalled
        let logs = self.console_logs.clone();
        let _ = client.send("Runtime.enable", json!({}));
        // 事件回调注册到 CdpClient
        // 由于 client 的 on 简化实现,这里用轮询方式:见 fetch_console
    }

    /// 获取控制台日志(过滤 + 截断 + 可选清空)
    pub fn get_console(
        &self,
        r#type: &str,
        search: &str,
        limit: usize,
        clear: bool,
    ) -> Vec<String> {
        let mut logs = self.console_logs.lock().unwrap();
        let filtered: Vec<String> = logs
            .iter()
            .filter(|l| r#type == "all" || l.r#type == r#type)
            .filter(|l| search.is_empty() || l.text.contains(search))
            .take(limit)
            .map(|l| l.text.clone())
            .collect();
        if clear {
            logs.clear();
        }
        filtered
    }

    /// 注册网络响应监听(需 Network.enable)
    pub fn attach_network(&self, client: &CdpClient) {
        let pending = self.pending_responses.clone();
        let _ = client.send("Network.enable", json!({}));
        // 事件在 client.on 注册,见 http.rs 初始化
    }
}

/// 全局事件分发:由 CdpClient 的事件监听器调用
/// 通过静态注册表关联到各 SessionState
pub struct EventHub {
    pub console_sinks: Arc<Mutex<Vec<Arc<Mutex<Vec<ConsoleEntry>>>>>>,
    pub response_sinks: Arc<Mutex<Vec<Arc<Mutex<Vec<PendingResponse>>>>>>,
}

impl crate::cdp::client::EventDispatcher for EventHub {
    fn dispatch(&self, method: &str, params: Value) {
        self.dispatch_event(method, params);
    }
}

impl EventHub {
    pub fn new() -> Self {
        Self {
            console_sinks: Arc::new(Mutex::new(Vec::new())),
            response_sinks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 处理 CDP 事件(由 CdpClient.on 注册的回调调用)
    pub fn dispatch_event(&self, method: &str, params: Value) {
        match method {
            "Runtime.consoleAPICalled" => {
                let type_ = params.get("type").and_then(|t| t.as_str()).unwrap_or("log");
                let args = params.get("args").and_then(|a| a.as_array()).cloned().unwrap_or_default();
                let text: String = args
                    .iter()
                    .map(|a| {
                        if let Some(v) = a.get("value") {
                            if let Some(s) = v.as_str() {
                                return s.to_string();
                            }
                            return v.to_string();
                        }
                        a.get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let entry = ConsoleEntry {
                    r#type: type_.to_string(),
                    text,
                };
                for sink in self.console_sinks.lock().unwrap().iter() {
                    sink.lock().unwrap().push(entry.clone());
                    // 限制大小
                    let mut s = sink.lock().unwrap();
                    if s.len() > 2000 {
                        let excess = s.len() - 2000;
                        s.drain(0..excess);
                    }
                }
            }
            "Network.responseReceived" => {
                let resp = params.get("response").cloned().unwrap_or(Value::Null);
                let url = resp.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
                let status = resp.get("status").and_then(|s| s.as_u64()).unwrap_or(0);
                for sink in self.response_sinks.lock().unwrap().iter() {
                    let mut vec = sink.lock().unwrap();
                    for p in vec.iter_mut() {
                        if !p.matched && url.contains(&p.pattern) {
                            p.matched = true;
                            p.result = Some(json!({ "matched": true, "url": url, "status": status }));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
