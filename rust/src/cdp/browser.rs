//! CDP 浏览器控制器:--app 启动 + 页面级操作
use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use super::client::CdpClient;

/// 元素盒模型
#[derive(Debug, Clone)]
pub struct BoxModel {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 浏览器启动选项
pub struct BrowserOptions {
    pub executable: String,
    pub user_data_dir: String,
    pub headless: bool,
}

/// 浏览器控制器
pub struct CdpBrowser {
    client: CdpClient,
    proc: Option<Child>,
    port: u16,
    target_id: Option<String>,
}

impl CdpBrowser {
    pub fn new() -> Self {
        Self {
            client: CdpClient::new(),
            proc: None,
            port: 0,
            target_id: None,
        }
    }

    pub fn client(&self) -> &CdpClient {
        &self.client
    }

    /// 设置全局事件分发器
    pub fn set_dispatcher(&mut self, dispatcher: Arc<dyn crate::cdp::client::EventDispatcher>) {
        self.client_mut().set_dispatcher(dispatcher);
    }

    /// 可变访问 client(仅内部用)
    fn client_mut(&mut self) -> &mut CdpClient {
        &mut self.client
    }

    /// 启动浏览器并连接页面 target
    pub async fn launch(&mut self, opts: &BrowserOptions, url: &str) -> Result<(), String> {
        self.port = pick_free_port().await?;
        let mut args = vec![
            format!("--app={url}"),
            format!("--remote-debugging-port={}", self.port),
            format!("--user-data-dir={}", opts.user_data_dir),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-networking".into(),
            "--no-sandbox".into(),
        ];
        if opts.headless {
            args.push("--headless=new".into());
        }

        let child = Command::new(&opts.executable)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn browser failed: {e}"))?;
        self.proc = Some(child);

        // 轮询调试端口
        let http_url = format!("http://127.0.0.1:{}", self.port);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut version: Option<Value> = None;
        while std::time::Instant::now() < deadline {
            if let Some(p) = &mut self.proc {
                if let Ok(Some(_)) = p.try_wait() {
                    return Err("Browser process exited early".into());
                }
            }
            if let Ok(resp) = reqwest::get(format!("{http_url}/json/version")).await {
                if let Ok(v) = resp.json::<Value>().await {
                    version = Some(v);
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let version = version.ok_or("Browser debug port not ready")?;

        // 先连浏览器级(用于发现 target)
        let browser_ws = version
            .get("webSocketDebuggerUrl")
            .and_then(|v| v.as_str())
            .ok_or("no webSocketDebuggerUrl")?;
        self.client.connect(browser_ws).await?;

        // 找页面 target
        let page_ws = self.wait_for_page_target().await?;
        // 重连到页面 target
        self.client.connect(&page_ws).await?;
        let _ = self.client.send("Page.enable", json!({})).await;

        // --app=URL 时等待页面就绪
        if url != "about:blank" {
            self.wait_for_url(url, 15000).await;
        }
        Ok(())
    }

    /// 等待页面 target 出现
    async fn wait_for_page_target(&self) -> Result<String, String> {
        let http_url = format!("http://127.0.0.1:{}", self.port);
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            if let Ok(resp) = reqwest::get(format!("{http_url}/json/list")).await {
                if let Ok(list) = resp.json::<Value>().await {
                    if let Some(arr) = list.as_array() {
                        for t in arr {
                            let ttype = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            let turl = t.get("url").and_then(|v| v.as_str()).unwrap_or("");
                            if ttype == "page" && !turl.starts_with("devtools://") {
                                let ws = t
                                    .get("webSocketDebuggerUrl")
                                    .and_then(|v| v.as_str())
                                    .ok_or("page target no ws")?
                                    .to_string();
                                return Ok(ws);
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        Err("No page target found".into())
    }

    /// 等待页面加载到指定 URL(超时静默)
    async fn wait_for_url(&self, url: &str, timeout_ms: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        let prefix = url.split('#').next().unwrap_or(url);
        while std::time::Instant::now() < deadline {
            if let Ok(cur) = self.evaluate("location.href").await {
                if let Some(cur) = cur.as_str() {
                    if cur.starts_with(prefix) {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// 导航到 URL
    pub async fn navigate(&self, url: &str) -> Result<(), String> {
        self.client.send("Page.enable", json!({})).await?;
        let _ = self.client
            .send("Page.navigate", json!({ "url": url }))
            .await?;
        Ok(())
    }

    /// 执行 JS 返回结果
    pub async fn evaluate(&self, expression: &str) -> Result<Value, String> {
        let r = self
            .client
            .send(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(exc) = r.get("exceptionDetails") {
            let text = exc
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("evaluate error");
            return Err(text.to_string());
        }
        Ok(r.get("result").and_then(|v| v.get("value")).cloned().unwrap_or(Value::Null))
    }

    /// 获取元素盒模型(用于覆盖层高亮)
    pub async fn get_box_model(&self, selector: &str) -> Result<Option<BoxModel>, String> {
        let expr = format!(
            "(() => {{ const el = document.querySelector({sel}); if (!el) return null; \
             const r = el.getBoundingClientRect(); \
             return {{ x: r.x, y: r.y, width: r.width, height: r.height }}; }})()",
            sel = serde_json::to_string(selector).unwrap_or_default()
        );
        let v = self.evaluate(&expr).await?;
        if v.is_null() {
            return Ok(None);
        }
        Ok(Some(BoxModel {
            x: v.get("x").and_then(|x| x.as_f64()).unwrap_or(0.0),
            y: v.get("y").and_then(|y| y.as_f64()).unwrap_or(0.0),
            width: v.get("width").and_then(|w| w.as_f64()).unwrap_or(0.0),
            height: v.get("height").and_then(|h| h.as_f64()).unwrap_or(0.0),
        }))
    }

    /// 页面截图(base64)
    pub async fn screenshot(&self) -> Result<String, String> {
        let r = self
            .client
            .send("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        r.get("data")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or("screenshot no data".into())
    }

    /// 当前 URL
    pub async fn current_url(&self) -> Result<String, String> {
        let v = self.evaluate("location.href").await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// 可见文本
    pub async fn visible_text(&self) -> Result<String, String> {
        let v = self
            .evaluate("document.body ? document.body.innerText : ''")
            .await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// 关闭浏览器
    pub fn close(&mut self) {
        if let Some(mut p) = self.proc.take() {
            let _ = p.kill();
            let _ = p.wait();
        }
        self.target_id = None;
    }

    /// 浏览器进程 PID(用于查找窗口句柄)
    pub fn pid(&self) -> Option<u32> {
        self.proc.as_ref().map(|p| p.id())
    }
}

/// 借用系统临时端口
async fn pick_free_port() -> Result<u16, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("pick port failed: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local addr failed: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}
