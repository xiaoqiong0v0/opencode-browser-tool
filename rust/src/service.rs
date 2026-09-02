//! 应用核心:HTTP 端点分发(基于 Tauri Webview 控制层)
//! 所有操作通过 control 模块 eval 驱动页面 Webview
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri::Manager;

use crate::control;
use crate::ui;

/// 应用状态(Arc 共享)
pub struct App {
    /// Tauri 应用句柄(驱动页面/覆盖层/面板 Webview)
    pub handle: AppHandle,
    /// 浏览器数据目录(兼容参数,WebView 模式下不下载浏览器)
    pub browsers_path: String,
}

impl App {
    pub fn new(handle: AppHandle, browsers_path: String) -> Self {
        Self { handle, browsers_path }
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
            "/api/clear" => self.clear(&body).await,
            "/api/select" => self.select(&body).await,
            "/api/hover" => self.hover(&body).await,
            "/api/press-key" => self.press_key(&body).await,
            "/api/drag" => self.drag(&body).await,
            "/api/upload-file" => self.upload_file(&body).await,
            "/api/screenshot" => self.screenshot(&body).await,
            "/api/evaluate" => self.evaluate(&body).await,
            "/api/visible-text" => self.visible_text().await,
            "/api/visible-html" => self.visible_html().await,
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
            "/api/notify" => self.notify(&body).await,
            "/api/pdf" => self.pdf().await,
            "/api/user-agent" => self.set_user_agent(&body).await,
            "/api/panel-config" => Ok(json!({ "ok": true })),
            "/api/close-session" => Ok(json!({ "closed": true })),
            "/api/accessibility" => self.accessibility().await,
            "/api/iframe-click" => self.iframe_click(&body).await,
            "/api/iframe-fill" => self.iframe_fill(&body).await,
            "/api/tabs/new" => self.tabs_new(&body).await,
            "/api/tabs/switch" => self.tabs_switch(&body).await,
            "/api/tabs/close" => self.tabs_close(&body).await,
            "/api/click-switch-tab" => self.click_switch_tab(&body).await,
            // 面板相关(批注状态机)
            "/api/annotate/toggle" => self.annotate_toggle().await,
            "/api/annotate/records" => self.annotate_records().await,
            "/api/annotate/send" => self.annotate_send().await,
            "/api/annotate/consume-sent" => self.annotate_consume_sent().await,
            // 设备预设与开发者工具
            "/api/device" => self.device(&body).await,
            "/api/device/list" => self.device_list().await,
            "/api/devtools" => self.devtools(&body).await,
            // 媒体设备模式(simulate=模拟 / real=真实)
            "/api/media/mode" => self.media_mode(&body).await,
            // fake 麦克风音频注入(模拟音频输入)
            "/api/media/audio" => self.media_audio(&body).await,
            // fake 摄像头画面注入(模拟视频输入)
            "/api/media/video" => self.media_video(&body).await,
            _ => Err(format!("Not found: POST {url}")),
        }
    }

    /// 在 tokio 线程中执行同步控制操作(避免阻塞 worker)
    fn run<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
    {
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| f(&handle))
    }

    // ---- 端点实现(全部通过 control 驱动页面 Webview) ----

    async fn status(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        let st = tokio::task::block_in_place(|| {
            let url = control::page_state(&handle)?;
            let title = url["title"].as_str().unwrap_or("").to_string();
            let cur = url["url"].as_str().unwrap_or("").to_string();
            Ok::<_, String>((cur, title))
        })?;
        let tabs_count = self.handle.state::<crate::ui::UiState>().tabs.lock().unwrap().len();
        Ok(json!({ "open": true, "url": st.0, "title": st.1, "tabs": tabs_count, "installing": {} }))
    }

    async fn navigate(&self, body: &Value) -> Result<Value, String> {
        let url = body.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            return Err("url is required".into());
        }
        // 裸域名自动补 http://
        let url = ui::normalize_url(&url);
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::navigate(&handle, &url))?;
        // 同步激活标签的 URL(工具栏显示一致)
        ui::sync_active_tab(&self.handle, &url, "");
        Ok(json!({ "url": url }))
    }

    /// 在页面显示通知气泡(通过覆盖层 Webview 的 __btOverlay.notify)
    async fn notify(&self, body: &Value) -> Result<Value, String> {
        let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ty = body.get("type").and_then(|v| v.as_str()).unwrap_or("ok").to_string();
        if message.is_empty() {
            return Err("message is required".into());
        }
        let handle = self.handle.clone();
        let js = format!(
            "(function(){{ var o = window.__btOverlay; if(!o) return {{error:'overlay not ready'}}; o.notify({m:?},{t:?}); return {{ok:true}}; }})()",
            m = message,
            t = ty
        );
        tokio::task::block_in_place(|| {
            let raw = ui::eval_overlay(&handle, &js)?;
            // eval_overlay 用 eval() 无返回值通道,失败仅返回 Err;success 分支直接返回
            let _ = raw;
            Ok::<(), String>(())
        })?;
        Ok(json!({ "notified": true }))
    }

    /// 关闭浏览器:退出整个应用(触发 ExitRequested → 进程退出)
    async fn close(&self) -> Result<Value, String> {
        self.handle.exit(0);
        Ok(json!({ "closed": true }))
    }

    async fn click(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::click(&handle, &s))?;
        Ok(json!({ "clicked": true }))
    }

    async fn fill(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let v = body.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::fill(&handle, &s, &v))?;
        Ok(json!({ "filled": true }))
    }

    async fn clear(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::fill(&handle, &s, ""))?;
        Ok(json!({ "cleared": true }))
    }

    async fn select(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let v = body.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el) return {{error:"not found"}};
              el.value = {v:?};
              el.dispatchEvent(new Event("change", {{bubbles:true}}));
              return {{ok:true}};
            }})()"#,
            s = s,
            v = v
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if r.get("error").is_some() {
            return Err(r["error"].as_str().unwrap_or("select failed").to_string());
        }
        Ok(json!({ "selected": true }))
    }

    async fn hover(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el) return {{error:"not found"}};
              el.dispatchEvent(new MouseEvent("mouseover", {{bubbles:true}}));
              el.dispatchEvent(new MouseEvent("mouseenter", {{bubbles:true}}));
              return {{ok:true}};
            }})()"#,
            s = s
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if r.get("error").is_some() {
            return Err(r["error"].as_str().unwrap_or("hover failed").to_string());
        }
        Ok(json!({ "hovered": true }))
    }

    async fn press_key(&self, body: &Value) -> Result<Value, String> {
        let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        // 通过 eval 派发键盘事件(Enter/Escape 等常用键)
        let key_map: &[(&str, &str)] = &[
            ("Enter", "Enter"),
            ("Escape", "Escape"),
            ("Tab", "Tab"),
            ("Backspace", "Backspace"),
            ("ArrowUp", "ArrowUp"),
            ("ArrowDown", "ArrowDown"),
            ("ArrowLeft", "ArrowLeft"),
            ("ArrowRight", "ArrowRight"),
        ];
        let code = key_map
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&key))
            .map(|(_, c)| c.to_string())
            .unwrap_or_else(|| key.clone());
        let js = format!(
            r#"(function(){{
              const el = document.activeElement || document.body;
              el.dispatchEvent(new KeyboardEvent("keydown", {{key:{k:?},code:{k:?},bubbles:true}}));
              el.dispatchEvent(new KeyboardEvent("keypress", {{key:{k:?},code:{k:?},bubbles:true}}));
              el.dispatchEvent(new KeyboardEvent("keyup", {{key:{k:?},code:{k:?},bubbles:true}}));
              return {{ok:true}};
            }})()"#,
            k = code
        );
        tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        Ok(json!({ "pressed": key }))
    }

    async fn drag(&self, body: &Value) -> Result<Value, String> {
        let from = body.get("sourceSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let to = body.get("targetSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const src = document.querySelector({from:?});
              const dst = document.querySelector({to:?});
              if (!src || !dst) return {{error:"element not found"}};
              const dataTransfer = new DataTransfer();
              src.dispatchEvent(new DragEvent("dragstart", {{bubbles:true,dataTransfer}}));
              dst.dispatchEvent(new DragEvent("dragover", {{bubbles:true,dataTransfer}}));
              dst.dispatchEvent(new DragEvent("drop", {{bubbles:true,dataTransfer}}));
              src.dispatchEvent(new DragEvent("dragend", {{bubbles:true,dataTransfer}}));
              return {{ok:true}};
            }})()"#,
            from = from,
            to = to
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if r.get("error").is_some() {
            return Err(r["error"].as_str().unwrap_or("drag failed").to_string());
        }
        Ok(json!({ "dragged": true }))
    }

    async fn upload_file(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let path = body.get("filePath").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        // WebView 文件上传:通过 eval 创建 File 对象注入
        let js = format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el) return {{error:"not found"}};
              const path = {p:?};
              fetch("file:///"+path).then(r=>r.blob()).then(b=>{{
                const dt = new DataTransfer();
                dt.items.add(new File([b], path.split('/').pop()));
                el.files = dt.files;
                el.dispatchEvent(new Event("change", {{bubbles:true}}));
              }});
              return {{ok:true}};
            }})()"#,
            s = s,
            p = path
        );
        tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        Ok(json!({ "uploaded": true }))
    }

    async fn screenshot(&self, body: &Value) -> Result<Value, String> {
        let handle = self.handle.clone();
        let base64 = tokio::task::block_in_place(|| {
            crate::control::screenshot::screenshot(&handle)
        })?;
        let _ = body;
        Ok(json!({ "base64": base64, "mime": "image/png" }))
    }

    async fn evaluate(&self, body: &Value) -> Result<Value, String> {
        let script = body.get("script").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::evaluate(&handle, &script))?;
        Ok(r)
    }

    async fn visible_text(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        let t = tokio::task::block_in_place(|| control::visible_text(&handle))?;
        Ok(json!({ "text": t }))
    }

    async fn visible_html(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        let js = r#"(function(){ return document.body ? document.body.innerHTML : ""; })()"#;
        let r = tokio::task::block_in_place(|| control::eval(&handle, js))?;
        Ok(json!({ "html": r.as_str().unwrap_or("") }))
    }

    async fn element_state(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::element_state(&handle, &s))?;
        Ok(r)
    }

    async fn dropdown_options(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el || !el.options) return {{options:[]}};
              return {{options: Array.from(el.options).map(o => ({{value:o.value, text:o.text}}))}};
            }})()"#,
            s = s
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        Ok(r)
    }

    async fn wait_for_selector(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let timeout = body.get("timeout").and_then(|v| v.as_u64()).unwrap_or(10000);
        let handle = self.handle.clone();
        let found = tokio::task::block_in_place(|| control::wait_for_selector(&handle, &s, timeout))?;
        Ok(json!({ "found": found }))
    }

    async fn scroll(&self, body: &Value) -> Result<Value, String> {
        let dx = body.get("dx").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let dy = body.get("dy").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::scroll(&handle, dx, dy))?;
        Ok(json!({ "scrolled": true }))
    }

    async fn scroll_to_element(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::scroll_to_element(&handle, &s))?;
        Ok(json!({ "scrolled": true }))
    }

    async fn reload(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            ui::active_page_webview(&handle)
                .ok_or("page webview not ready")?
                .reload()
                .map_err(|e| format!("reload failed: {e}"))
        })?;
        Ok(json!({ "reloaded": true }))
    }

    async fn go_back(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            let js = "history.back(); true";
            control::eval(&handle, js)
        })?;
        // 等待导航生效后读取当前 URL
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let handle2 = self.handle.clone();
        let url = tokio::task::block_in_place(|| {
            let v = control::page_state(&handle2).unwrap_or_default();
            v["url"].as_str().unwrap_or("").to_string()
        });
        Ok(json!({ "back": true, "url": url }))
    }

    async fn go_forward(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            let js = "history.forward(); true";
            control::eval(&handle, js)
        })?;
        // 等待导航生效后读取当前 URL
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let handle2 = self.handle.clone();
        let url = tokio::task::block_in_place(|| {
            let v = control::page_state(&handle2).unwrap_or_default();
            v["url"].as_str().unwrap_or("").to_string()
        });
        Ok(json!({ "forward": true, "url": url }))
    }

    async fn resize(&self, body: &Value) -> Result<Value, String> {
        let w = body.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let h = body.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            let win = ui::active_page_webview(&handle)
                .ok_or("page webview not ready")?
                .window_ref()
                .clone();
            win.set_size(tauri::LogicalSize::new(w, h))
                .map_err(|e| format!("resize failed: {e}"))
        })?;
        Ok(json!({ "resized": true }))
    }

    /// 读取页面控制台日志(由页面桥 __btLogs 捕获,读取后清空)
    async fn console_logs(&self, _body: &Value) -> Result<Value, String> {
        let handle = self.handle.clone();
        let logs = tokio::task::block_in_place(|| {
            let v = control::eval(
                &handle,
                "(function(){var l=window.__btLogs||[];window.__btLogs=[];return l;})()",
            )?;
            Ok::<_, String>(v)
        })?;
        Ok(json!({ "logs": logs }))
    }

    /// 记录期望:清空响应历史并登记(声明"从此开始捕获")
    async fn expect_response(&self, body: &Value) -> Result<Value, String> {
        let pattern = body
            .get("urlPattern")
            .or_else(|| body.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        crate::control::responses::clear();
        Ok(json!({ "expected": true, "pattern": pattern }))
    }

    /// 断言响应:在已捕获历史中匹配 pattern(URL 子串),返回最近命中
    async fn assert_response(&self, body: &Value) -> Result<Value, String> {
        let pattern = body
            .get("id")
            .or_else(|| body.get("urlPattern"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if pattern.is_empty() {
            return Ok(json!({ "matched": false, "error": "no pattern provided" }));
        }
        match crate::control::responses::find(&pattern) {
            Some(entry) => Ok(json!({ "matched": true, "url": entry.url, "status": entry.status })),
            None => Ok(json!({ "matched": false, "error": "no response matched the pattern yet" })),
        }
    }

    async fn pdf(&self) -> Result<Value, String> {
        Err("pdf not supported on webview".into())
    }

    /// 运行时修改页面 User-Agent(Windows 通过 ICoreWebView2Settings2,其他平台报错)
    async fn set_user_agent(&self, body: &Value) -> Result<Value, String> {
        let ua = body.get("userAgent").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if ua.is_empty() {
            return Err("userAgent is required".into());
        }
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| crate::devices::set_user_agent(&handle, &ua))?;
        Ok(json!({ "set": true, "ua": ua }))
    }

    async fn accessibility(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        let tree =
            tokio::task::block_in_place(|| crate::control::accessibility::accessibility_tree(&handle))?;
        Ok(tree)
    }

    /// 标签列表(真实多标签,含 id/url/title)
    async fn tabs(&self) -> Result<Value, String> {
        let state = self.handle.state::<crate::ui::UiState>();
        let tabs = state.tabs.lock().unwrap().clone();
        let list: Vec<serde_json::Value> = tabs
            .iter()
            .map(|t| json!({ "id": t.id, "url": t.url, "title": t.title }))
            .collect();
        Ok(json!({ "tabs": list }))
    }

    /// 新建标签:真多标签独立 Webview 并导航(工具栏/+ / target=_blank 共用逻辑)
    async fn tabs_new(&self, body: &Value) -> Result<Value, String> {
        let url = body.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string();
        if url.is_empty() {
            return Err("url is required".into());
        }
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| crate::ui::open_new_tab(&handle, &url))?;
        Ok(json!({ "opened": true, "url": url }))
    }

    /// 切换标签(index:标签位置 0 起,映射到真实 tab id)
    async fn tabs_switch(&self, body: &Value) -> Result<Value, String> {
        let idx = body.get("index").and_then(|v| v.as_i64()).unwrap_or(0) as usize;
        let state = self.handle.state::<crate::ui::UiState>();
        let tabs = state.tabs.lock().unwrap();
        let tid = tabs.get(idx).map(|t| t.id).ok_or("tab index out of range")?;
        let url = tabs[idx].url.clone();
        drop(tabs);
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| crate::ui::switch_tab(&handle, tid))?;
        Ok(json!({ "switched": true, "url": url }))
    }

    /// 关闭标签(index 可选:不传/负数关闭当前激活标签)
    async fn tabs_close(&self, body: &Value) -> Result<Value, String> {
        let idx = body.get("index").and_then(|v| v.as_i64());
        let state = self.handle.state::<crate::ui::UiState>();
        let tabs = state.tabs.lock().unwrap();
        let tid = match idx {
            Some(i) if i >= 0 => tabs.get(i as usize).map(|t| t.id).ok_or("tab index out of range")?,
            _ => {
                // 不传 index:关闭当前激活标签
                let active = *state.active_tab.lock().unwrap();
                active
            }
        };
        drop(tabs);
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| crate::ui::close_tab(&handle, tid))?;
        Ok(json!({ "closed": true }))
    }

    /// 在 iframe 中点击元素
    async fn iframe_click(&self, body: &Value) -> Result<Value, String> {
        let iframe = body.get("iframeSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sel = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const f = document.querySelector({iframe:?});
              if (!f) return {{error:"iframe not found"}};
              const d = f.contentDocument;
              if (!d) return {{error:"cross-origin iframe not accessible"}};
              const el = d.querySelector({sel:?});
              if (!el) return {{error:"element not found"}};
              el.scrollIntoView({{block:"center"}});
              el.click();
              return {{ok:true}};
            }})()"#,
            iframe = iframe,
            sel = sel
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if r.get("error").is_some() {
            return Err(r["error"].as_str().unwrap_or("iframe click failed").to_string());
        }
        Ok(json!({ "clicked": true }))
    }

    /// 在 iframe 中填写输入框
    async fn iframe_fill(&self, body: &Value) -> Result<Value, String> {
        let iframe = body.get("iframeSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let sel = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let val = body.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const f = document.querySelector({iframe:?});
              if (!f) return {{error:"iframe not found"}};
              const d = f.contentDocument;
              if (!d) return {{error:"cross-origin iframe not accessible"}};
              const el = d.querySelector({sel:?});
              if (!el) return {{error:"element not found"}};
              const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
              const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
              setter.call(el, {val:?});
              el.dispatchEvent(new Event("input", {{bubbles:true}}));
              el.dispatchEvent(new Event("change", {{bubbles:true}}));
              return {{ok:true}};
            }})()"#,
            iframe = iframe,
            sel = sel,
            val = val
        );
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if r.get("error").is_some() {
            return Err(r["error"].as_str().unwrap_or("iframe fill failed").to_string());
        }
        Ok(json!({ "filled": true }))
    }

    async fn click_switch_tab(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| control::click(&handle, &s))?;
        // 点击后读取当前 URL(点击可能触发导航)
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let handle2 = self.handle.clone();
        let url = tokio::task::block_in_place(|| {
            let v = control::page_state(&handle2).unwrap_or_default();
            v["url"].as_str().unwrap_or("").to_string()
        });
        Ok(json!({ "clicked": true, "url": url }))
    }

    // ---- 批注状态机端点 ----

    /// 应用设备预设(窗口尺寸 + UA)
    async fn device(&self, body: &Value) -> Result<Value, String> {
        let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let device = crate::devices::find(&name).ok_or_else(|| format!("unknown device: {name}"))?;
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| crate::devices::apply(&handle, device))?;
        Ok(json!({
            "device": device.name,
            "width": device.width,
            "height": device.height,
            "ua": device.ua.unwrap_or("(default)"),
        }))
    }

    /// 列出所有设备预设
    async fn device_list(&self) -> Result<Value, String> {
        let list: Vec<Value> = crate::devices::DEVICES
            .iter()
            .map(|d| {
                json!({
                    "name": d.name,
                    "width": d.width,
                    "height": d.height,
                    "ua": d.ua.unwrap_or(""),
                })
            })
            .collect();
        Ok(json!({ "devices": list }))
    }

    /// 打开/关闭开发者工具
    /// 注:wry 的 is_devtools_open/close_devtools 在 webview2 上为空实现,
    /// 因此开关状态由 UiState 自行维护,close 仅标记状态(窗口需手动关闭)
    async fn devtools(&self, body: &Value) -> Result<Value, String> {
        let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("toggle").to_string();
        let state = self.handle.state::<crate::ui::UiState>();
        let mut open = state.devtools_open.lock().unwrap();
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            let page = ui::active_page_webview(&handle).ok_or("page webview not ready")?;
            match action.as_str() {
                "open" => {
                    if !*open {
                        page.open_devtools();
                        *open = true;
                    }
                }
                "close" => {
                    if *open {
                        page.close_devtools();
                        *open = false;
                    }
                }
                "toggle" => {
                    if *open {
                        page.close_devtools();
                        *open = false;
                    } else {
                        page.open_devtools();
                        *open = true;
                    }
                }
                other => return Err(format!("unknown action: {other} (open/close/toggle)")),
            }
            Ok::<(), String>(())
        })?;
        Ok(json!({ "open": *open }))
    }

    /// 查询/切换媒体设备模式(simulate=模拟设备 / real=真实设备)
    /// 带 mode:校验后更新 UiState 并同步所有页面脚本;不带 mode:返回当前模式
    async fn media_mode(&self, body: &Value) -> Result<Value, String> {
        match body.get("mode").and_then(|v| v.as_str()) {
            Some(m) if m == "simulate" || m == "real" => {
                let m2 = m.to_string();
                let handle = self.handle.clone();
                // 闭包 move 捕获,m2 保留在外部供返回
                let mode = m2.clone();
                tokio::task::block_in_place(move || {
                    let state = handle.state::<crate::ui::UiState>();
                    *state.media_mode.lock().unwrap() = mode.clone();
                    // 同步所有页面 webview 的 __btMediaMode(切换标签后模式保持一致)
                    let labels: Vec<String> = state
                        .tabs
                        .lock()
                        .unwrap()
                        .iter()
                        .filter_map(|t| t.webview.clone())
                        .collect();
                    for label in labels {
                        if let Some(w) = handle.get_webview(&label) {
                            let _ = w.eval(&format!("window.__btMediaMode = {mode:?}"));
                        }
                    }
                });
                Ok(json!({ "mode": m2 }))
            }
            None => {
                let cur = self
                    .handle
                    .state::<crate::ui::UiState>()
                    .media_mode
                    .lock()
                    .unwrap()
                    .clone();
                Ok(json!({ "mode": cur }))
            }
            Some(other) => Err(format!("invalid mode: {other} (simulate/real)")),
        }
    }

    /// 向页面 fake 麦克风注入声音(模拟音频输入)
    /// body: { kind: tone|seq|dtmf|noise|audio|ambient|stop, ... }
    async fn media_audio(&self, body: &Value) -> Result<Value, String> {
        let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let js = match kind.as_str() {
            "tone" => {
                let freq = body.get("freq").and_then(|v| v.as_f64()).unwrap_or(440.0);
                let dur = body.get("durMs").and_then(|v| v.as_f64()).unwrap_or(200.0);
                format!("(window.__btFakeMic?window.__btFakeMic.tone({freq},{dur}):0)")
            }
            "seq" => {
                let notes = body.get("notes").cloned().unwrap_or_else(|| json!([]));
                format!("(window.__btFakeMic?window.__btFakeMic.seq({notes}):0)")
            }
            "dtmf" => {
                let digits = body.get("digits").and_then(|v| v.as_str()).unwrap_or("").to_string();
                format!("(window.__btFakeMic?window.__btFakeMic.dtmf({digits:?}):0)")
            }
            "noise" => {
                let dur = body.get("durMs").and_then(|v| v.as_f64()).unwrap_or(400.0);
                format!("(window.__btFakeMic?window.__btFakeMic.noise({dur}):0)")
            }
            "audio" => {
                let data = body.get("data").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if data.is_empty() {
                    return Err("data (base64 audio) is required for kind=audio".into());
                }
                // loop=true 循环播放,false(默认)单次
                let loop_ = body.get("loop").and_then(|v| v.as_bool()).unwrap_or(false);
                format!("(window.__btFakeMic?window.__btFakeMic.inject({data:?},{loop_}):0)")
            }
            "ambient" => "(window.__btFakeMic?window.__btFakeMic.ambient():0)".to_string(),
            "stop" => "(window.__btFakeMic?window.__btFakeMic.stop():0)".to_string(),
            "" => return Err("kind is required (tone/seq/dtmf/noise/audio/ambient/stop)".into()),
            other => {
                return Err(format!("unknown kind: {other} (tone/seq/dtmf/noise/audio/ambient/stop)"))
            }
        };
        // 同步所有页面 webview(切换标签后注入目标保持一致)
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            let state = handle.state::<crate::ui::UiState>();
            let labels: Vec<String> = state
                .tabs
                .lock()
                .unwrap()
                .iter()
                .filter_map(|t| t.webview.clone())
                .collect();
            for label in labels {
                if let Some(w) = handle.get_webview(&label) {
                    let _ = w.eval(&js);
                }
            }
        });
        Ok(json!({ "injected": kind }))
    }

    /// 向页面 fake 摄像头注入画面(模拟视频输入)
    /// body: { kind: image|video|auto|stop, ... }
    async fn media_video(&self, body: &Value) -> Result<Value, String> {
        let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let js = match kind.as_str() {
            "image" => {
                let data = body.get("data").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if data.is_empty() {
                    return Err("data (base64 image) is required for kind=image".into());
                }
                format!("(window.__btFakeCam?window.__btFakeCam.setImage({data:?}):0)")
            }
            "video" => {
                let url = body.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if url.is_empty() {
                    return Err("url is required for kind=video".into());
                }
                // loop=true(默认)循环播放,false 播完冻结末帧
                let loop_ = body.get("loop").and_then(|v| v.as_bool()).unwrap_or(true);
                format!("(window.__btFakeCam?window.__btFakeCam.setVideo({url:?},{loop_}):0)")
            }
            "auto" => "(window.__btFakeCam?window.__btFakeCam.setAuto():0)".to_string(),
            "stop" => "(window.__btFakeCam?window.__btFakeCam.stop():0)".to_string(),
            "" => return Err("kind is required (image/video/auto/stop)".into()),
            other => return Err(format!("unknown kind: {other} (image/video/auto/stop)")),
        };
        // 同步所有页面 webview(切换标签后注入目标保持一致)
        let handle = self.handle.clone();
        tokio::task::block_in_place(move || {
            let state = handle.state::<crate::ui::UiState>();
            let labels: Vec<String> = state
                .tabs
                .lock()
                .unwrap()
                .iter()
                .filter_map(|t| t.webview.clone())
                .collect();
            for label in labels {
                if let Some(w) = handle.get_webview(&label) {
                    let _ = w.eval(&js);
                }
            }
        });
        Ok(json!({ "injected": kind }))
    }

    /// 批注模式开关(需 block_in_place:webview show/hide 需主线程,直接调用会与 tokio 死锁)
    async fn annotate_toggle(&self) -> Result<Value, String> {
        let state = self.handle.state::<crate::ui::UiState>();
        let on = tokio::task::block_in_place(|| {
            crate::ui::annotate::Annotator::toggle(&self.handle, &state)
        })?;
        Ok(json!({ "annotate": on }))
    }

    async fn annotate_records(&self) -> Result<Value, String> {
        let state = self.handle.state::<crate::ui::UiState>();
        let records = state.records.lock().unwrap().clone();
        Ok(json!(records))
    }

    async fn annotate_send(&self) -> Result<Value, String> {
        // 将当前记录快照放入待发送队列(插件端轮询 consume 拉取)
        let count = crate::ui::send_all_records(&self.handle);
        Ok(json!({ "count": count }))
    }

    async fn annotate_consume_sent(&self) -> Result<Value, String> {
        let state = self.handle.state::<crate::ui::UiState>();
        // 返回自上次消费后标记的记录
        let mut sent = state.sent_records.lock().unwrap();
        let items: Vec<serde_json::Value> = sent.drain(..).collect();
        Ok(json!({ "records": items }))
    }
}

/// 提取字符串字段
#[allow(dead_code)]
fn str(body: &Value, key: &str) -> String {
    body.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}
