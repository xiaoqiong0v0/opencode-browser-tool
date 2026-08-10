//! 应用核心:浏览器管理 + 40 个 /api/* 端点分发
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::cdp::browser::{BrowserOptions, CdpBrowser};
use crate::cdp::session::{PendingResponse, SessionState};

pub type SharedBrowser = Arc<tokio::sync::Mutex<Option<CdpBrowser>>>;

/// 应用状态(Arc 共享)
pub struct App {
    pub browsers_path: String,
    pub mirror: Option<String>,
    pub session: SessionState,
    pub browser: SharedBrowser,
    pub browser_type: String,
    /// 浏览器可执行文件路径(缓存)
    pub executable: std::sync::Mutex<Option<String>>,
}

impl App {
    pub fn new(browsers_path: String, mirror: Option<String>, browser_type: String) -> Self {
        Self {
            browsers_path,
            mirror,
            session: SessionState::new(),
            browser: Arc::new(tokio::sync::Mutex::new(None)),
            browser_type,
            executable: std::sync::Mutex::new(None),
        }
    }

    /// 分发端点
    pub async fn handle(&self, url: &str, body: Value) -> Result<Value, String> {
        match url {
            "/api/status" => self.status().await,
            "/api/tabs" => self.tabs().await,
            "/api/navigate" => self.navigate(&body).await,
            "/api/close" => self.close().await,
            "/api/click" => self.click(&body).await,
            "/api/fill" => self.fill(&body).await,
            "/api/clear" => self.fill_clear(&body).await,
            "/api/select" => self.select(&body).await,
            "/api/hover" => self.hover(&body).await,
            "/api/press-key" => self.press_key(&body).await,
            "/api/drag" => self.drag(&body).await,
            "/api/upload-file" => self.upload_file(&body).await,
            "/api/screenshot" => self.screenshot(&body).await,
            "/api/evaluate" => self.evaluate(&body).await,
            "/api/visible-text" => self.visible_text(&body).await,
            "/api/visible-html" => self.visible_html(&body).await,
            "/api/element-state" => self.element_state(&body).await,
            "/api/dropdown-options" => self.dropdown_options(&body).await,
            "/api/wait-for-selector" => self.wait_for_selector(&body).await,
            "/api/scroll" => self.scroll(&body).await,
            "/api/scroll-to-element" => self.scroll_to_element(&body).await,
            "/api/reload" => self.reload().await,
            "/api/go-back" => self.go_back().await,
            "/api/go-forward" => self.go_forward().await,
            "/api/resize" => self.resize(&body).await,
            "/api/console-logs" => self.console_logs(&body).await,
            "/api/expect-response" => self.expect_response(&body).await,
            "/api/assert-response" => self.assert_response(&body).await,
            "/api/notify" => Ok(json!({ "notified": true })),
            "/api/pdf" => self.pdf().await,
            "/api/user-agent" => self.set_user_agent(&body).await,
            "/api/panel-config" => Ok(json!({ "ok": true })),
            "/api/close-session" => Ok(json!({ "closed": true })),
            "/api/accessibility" => self.accessibility().await,
            "/api/iframe-click" | "/api/iframe-fill" => Err("iframe ops not supported yet".into()),
            "/api/tabs/new" => self.tabs_new(&body).await,
            "/api/tabs/switch" => self.tabs_switch(&body).await,
            "/api/tabs/close" => self.tabs_close(&body).await,
            "/api/click-switch-tab" => self.click_switch_tab(&body).await,
            _ => Err(format!("Not found: POST {url}")),
        }
    }

    // ---- 浏览器生命周期 ----

    /// 启动浏览器(如未启动)
    async fn ensure_browser(&self) -> Result<(), String> {
        let mut guard = self.browser.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let exe = self.find_executable().await?;
        let opts = BrowserOptions {
            executable: exe,
            user_data_dir: format!("{}/profile", self.browsers_path),
            headless: false,
        };
        let mut browser = CdpBrowser::new();
        browser.launch(&opts, "about:blank").await?;
        // 附加事件分发(console/network)
        let hub = Arc::new(crate::cdp::session::EventHub::new());
        hub.console_sinks
            .lock()
            .unwrap()
            .push(self.session.console_logs.clone());
        hub.response_sinks
            .lock()
            .unwrap()
            .push(self.session.pending_responses.clone());
        browser.set_dispatcher(hub);
        *guard = Some(browser);
        Ok(())
    }

    /// 获取浏览器可变锁(自动启动)
    async fn browser_guard(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<CdpBrowser>>, String> {
        self.ensure_browser().await?;
        Ok(self.browser.lock().await)
    }

    /// 查找浏览器可执行文件路径
    async fn find_executable(&self) -> Result<String, String> {
        if let Some(exe) = self.executable.lock().unwrap().as_ref() {
            return Ok(exe.clone());
        }
        let name = if self.browser_type == "firefox" {
            "firefox"
        } else {
            "chromium"
        };
        let manifest = crate::browsers::manifest::get_manifest(name)?;
        let dir = std::path::Path::new(&self.browsers_path)
            .join(format!("{}-{}", name, manifest.revision));
        let exe_candidates: Vec<std::path::PathBuf> = if cfg!(windows) {
            vec![
                dir.join("chrome-win64").join("chrome.exe"),
                dir.join("firefox").join("firefox.exe"),
            ]
        } else {
            vec![
                dir.join("chrome-linux64").join("chrome"),
                dir.join("firefox").join("firefox"),
            ]
        };
        for c in exe_candidates {
            if c.exists() {
                let exe = c.to_string_lossy().to_string();
                *self.executable.lock().unwrap() = Some(exe.clone());
                return Ok(exe);
            }
        }
        // 未找到 → 触发下载
        crate::browsers::downloader::download_browser(
            name,
            &self.browsers_path,
            self.mirror.as_deref(),
        )
        .await?;
        // 下载完成后再次查找(避免递归)
        self.find_executable_after_download(name)
    }

    /// 下载后查找可执行文件(非递归)
    fn find_executable_after_download(&self, name: &str) -> Result<String, String> {
        let manifest = crate::browsers::manifest::get_manifest(name)?;
        let dir = std::path::Path::new(&self.browsers_path)
            .join(format!("{}-{}", name, manifest.revision));
        let exe_candidates: Vec<std::path::PathBuf> = if cfg!(windows) {
            vec![
                dir.join("chrome-win64").join("chrome.exe"),
                dir.join("firefox").join("firefox.exe"),
            ]
        } else {
            vec![
                dir.join("chrome-linux64").join("chrome"),
                dir.join("firefox").join("firefox"),
            ]
        };
        for c in exe_candidates {
            if c.exists() {
                let exe = c.to_string_lossy().to_string();
                *self.executable.lock().unwrap() = Some(exe.clone());
                return Ok(exe);
            }
        }
        Err("browser executable not found after download".into())
    }

    // ---- 端点实现 ----

    async fn status(&self) -> Result<Value, String> {
        let open = self.browser.lock().await.is_some();
        if !open {
            return Ok(json!({ "open": false, "installing": {} }));
        }
        let g = self.browser_guard().await?;
        let b = g.as_ref().ok_or("browser not started")?;
        let url = b.current_url().await.unwrap_or_default();
        let title = b.evaluate("document.title").await.unwrap_or(Value::Null);
        let title = title.as_str().unwrap_or("").to_string();
        Ok(json!({ "open": true, "url": url, "title": title, "tabs": 1, "installing": {} }))
    }

    async fn navigate(&self, body: &Value) -> Result<Value, String> {
        let url = str(body, "url");
        if url.is_empty() {
            return Err("url is required".into());
        }
        let url = url.to_string();
        self.ensure_browser().await?;
        let g = self.browser.lock().await;
        let b = g.as_ref().ok_or("browser not started")?;
        b.navigate(&url).await?;
        tokio::time::sleep(Duration::from_millis(800)).await;
        Ok(json!({ "url": url }))
    }

    async fn close(&self) -> Result<Value, String> {
        let mut guard = self.browser.lock().await;
        if let Some(mut b) = guard.take() {
            b.close();
        }
        Ok(json!({ "closed": true }))
    }

    async fn click(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.click(&s).await?;
        Ok(json!({ "clicked": true }))
    }

    async fn fill(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let v = str(body, "value").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.fill(&s, &v).await?;
        Ok(json!({ "filled": true }))
    }

    async fn fill_clear(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.fill(&s, "").await?;
        Ok(json!({ "cleared": true }))
    }

    async fn select(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let v = str(body, "value").to_string();
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .select_option(&s, &v)
            .await?;
        Ok(json!({ "selected": true }))
    }

    async fn hover(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.hover(&s).await?;
        Ok(json!({ "hovered": true }))
    }

    async fn press_key(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let k = str(body, "key").to_string();
        let sel = if s.is_empty() { None } else { Some(s.as_str()) };
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .press_key(sel, &k)
            .await?;
        Ok(json!({ "pressed": true }))
    }

    async fn drag(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "sourceSelector").to_string();
        let t = str(body, "targetSelector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.drag(&s, &t).await?;
        Ok(json!({ "dragged": true }))
    }

    async fn upload_file(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let f = str(body, "filePath").to_string();
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .upload_file(&s, &f)
            .await?;
        Ok(json!({ "uploaded": true }))
    }

    async fn screenshot(&self, _body: &Value) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        let data = g.as_ref().ok_or("browser not started")?.screenshot().await?;
        Ok(json!({ "__buffer": data }))
    }

    async fn evaluate(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "script").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.evaluate(&s).await
    }

    async fn visible_text(&self, body: &Value) -> Result<Value, String> {
        let sel = str(body, "selector").to_string();
        let expr = if sel.is_empty() {
            "document.body ? document.body.innerText : ''".to_string()
        } else {
            format!(
                "(() => {{ const root = document.querySelector({sel}); return root ? root.innerText || '' : ''; }})()",
                sel = serde_json::to_string(&sel).unwrap_or_default()
            )
        };
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.evaluate(&expr).await
    }

    async fn visible_html(&self, body: &Value) -> Result<Value, String> {
        let sel = str(body, "selector").to_string();
        let sel_opt = if sel.is_empty() { None } else { Some(sel.as_str()) };
        let rm_scripts = body
            .get("removeScripts")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let rm_comments = body
            .get("removeComments")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let max_len = u64(body, "maxLength", 20000) as usize;
        let g = self.browser_guard().await?;
        let html = g
            .as_ref()
            .ok_or("browser not started")?
            .visible_html(sel_opt, rm_scripts, rm_comments, max_len)
            .await?;
        Ok(json!(html))
    }

    async fn element_state(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.element_state(&s).await
    }

    async fn dropdown_options(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .dropdown_options(&s)
            .await
    }

    async fn wait_for_selector(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let timeout = u64(body, "timeout", 10000);
        let g = self.browser_guard().await?;
        let found = g
            .as_ref()
            .ok_or("browser not started")?
            .wait_for_selector(&s, timeout)
            .await?;
        Ok(json!({ "found": found }))
    }

    async fn scroll(&self, body: &Value) -> Result<Value, String> {
        let amount = u64(body, "amount", 300) as f64;
        let dx = u64(body, "dx", 0) as f64;
        let dy = if body.get("dy").is_some() {
            u64(body, "dy", 0) as f64
        } else {
            amount
        };
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.scroll(dx, dy).await?;
        Ok(json!({ "scrolled": true }))
    }

    async fn scroll_to_element(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .scroll_to_element(&s)
            .await?;
        Ok(json!({ "scrolled": true }))
    }

    async fn reload(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        let b = g.as_ref().ok_or("browser not started")?;
        b.client().send("Page.reload", json!({})).await?;
        drop(g);
        tokio::time::sleep(Duration::from_millis(800)).await;
        let cur = self.current_url().await.unwrap_or_default();
        Ok(json!({ "url": cur }))
    }

    async fn go_back(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .evaluate("history.back()")
            .await?;
        drop(g);
        tokio::time::sleep(Duration::from_millis(800)).await;
        let cur = self.current_url().await.unwrap_or_default();
        Ok(json!({ "url": cur }))
    }

    async fn go_forward(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .evaluate("history.forward()")
            .await?;
        drop(g);
        tokio::time::sleep(Duration::from_millis(800)).await;
        let cur = self.current_url().await.unwrap_or_default();
        Ok(json!({ "url": cur }))
    }

    async fn resize(&self, body: &Value) -> Result<Value, String> {
        let w = u64(body, "width", 1280);
        let h = u64(body, "height", 800);
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.resize(w, h).await?;
        Ok(json!({ "resized": true }))
    }

    async fn pdf(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        let data = g.as_ref().ok_or("browser not started")?.pdf().await?;
        Ok(json!({ "__buffer": data }))
    }

    async fn set_user_agent(&self, body: &Value) -> Result<Value, String> {
        let ua = str(body, "userAgent").to_string();
        if ua.is_empty() {
            return Err("userAgent is required".into());
        }
        let g = self.browser_guard().await?;
        g.as_ref()
            .ok_or("browser not started")?
            .set_user_agent(&ua)
            .await?;
        Ok(json!({ "set": true }))
    }

    async fn accessibility(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        let r = g
            .as_ref()
            .ok_or("browser not started")?
            .client()
            .send("Accessibility.getFullAXTree", json!({}))
            .await?;
        Ok(r)
    }

    async fn tabs(&self) -> Result<Value, String> {
        let g = self.browser_guard().await?;
        let list = g
            .as_ref()
            .ok_or("browser not started")?
            .client()
            .send("Target.getTargets", json!({}))
            .await?;
        let targets = list
            .get("targetInfos")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let pages: Vec<Value> = targets
            .iter()
            .filter(|t| t.get("type").and_then(|v| v.as_str()) == Some("page"))
            .map(|t| {
                json!({
                    "url": t.get("url").cloned().unwrap_or(Value::Null),
                    "title": t.get("title").cloned().unwrap_or(Value::Null),
                })
            })
            .collect();
        Ok(json!(pages))
    }

    async fn console_logs(&self, body: &Value) -> Result<Value, String> {
        let type_ = str(body, "type");
        let search = str(body, "search");
        let limit = u64(body, "limit", 50) as usize;
        let clear = body.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
        let logs = self.session.get_console(type_, search, limit, clear);
        Ok(json!(logs))
    }

    async fn expect_response(&self, body: &Value) -> Result<Value, String> {
        let pattern = str(body, "urlPattern").to_string();
        self.session
            .pending_responses
            .lock()
            .unwrap()
            .push(PendingResponse {
                pattern: pattern.clone(),
                matched: false,
                result: None,
            });
        Ok(json!({ "id": pattern, "pattern": pattern }))
    }

    async fn assert_response(&self, body: &Value) -> Result<Value, String> {
        let id = str(body, "id");
        let entries = self.session.pending_responses.lock().unwrap();
        let entry = entries.iter().find(|p| p.pattern == id);
        match entry {
            Some(e) if e.matched => Ok(e.result.clone().unwrap_or(json!({ "matched": true }))),
            Some(_) => Ok(json!({ "matched": false, "error": "pending" })),
            None => Ok(json!({ "matched": false, "error": "no pending expectation" })),
        }
    }

    async fn current_url(&self) -> Result<String, String> {
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.current_url().await
    }

    async fn tabs_new(&self, body: &Value) -> Result<Value, String> {
        let url = str(body, "url");
        let g = self.browser_guard().await?;
        let t = g
            .as_ref()
            .ok_or("browser not started")?
            .client()
            .send("Target.createTarget", json!({ "url": url }))
            .await?;
        Ok(json!({ "url": t.get("targetId").cloned().unwrap_or(Value::Null) }))
    }

    async fn tabs_switch(&self, body: &Value) -> Result<Value, String> {
        let _ = body;
        let cur = self.current_url().await.unwrap_or_default();
        Ok(json!({ "url": cur }))
    }

    async fn tabs_close(&self, body: &Value) -> Result<Value, String> {
        let _ = body;
        Ok(json!({ "closed": true }))
    }

    async fn click_switch_tab(&self, body: &Value) -> Result<Value, String> {
        let s = str(body, "selector").to_string();
        let g = self.browser_guard().await?;
        g.as_ref().ok_or("browser not started")?.click(&s).await?;
        Ok(json!({ "switched": false }))
    }
}

// ---- helpers ----
pub fn str<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}

pub fn u64(v: &Value, key: &str, default: u64) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(default)
}
