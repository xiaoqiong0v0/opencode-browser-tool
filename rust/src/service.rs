//! 应用核心:HTTP 端点分发(基于 Tauri Webview 控制层)
//! 所有操作通过 control 模块 eval 驱动页面 Webview
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri::Manager;

use crate::control;
use crate::ui;

/// 滚动缺省方向(与 bt_cli `scroll` 用法一致)
const DEFAULT_SCROLL_DIRECTION: &str = "down";
/// 滚动缺省像素
const DEFAULT_SCROLL_AMOUNT: i64 = 300;
/// visible-html 默认截断长度(字符)
const DEFAULT_HTML_MAX_LENGTH: u64 = 20000;
/// visible-html 默认移除 <script>
const DEFAULT_REMOVE_SCRIPTS: bool = true;
/// visible-html 默认保留 HTML 注释
const DEFAULT_REMOVE_COMMENTS: bool = false;
/// console-logs 默认返回条数(取最后 N 条)
const DEFAULT_CONSOLE_LOG_LIMIT: u64 = 50;

/// visible-html 提取 JS(占位符由 Rust 侧替换;返回 {html,truncated} 或 {error})
/// 先 clone 再删除,不改动页面实际 DOM
const VISIBLE_HTML_JS: &str = r##"
(function(){
  try{
    var sel=__BT_SELECTOR__;
    var removeScripts=__BT_REMOVE_SCRIPTS__;
    var removeComments=__BT_REMOVE_COMMENTS__;
    var maxLength=__BT_MAX_LENGTH__;
    var root=sel?document.querySelector(sel):document.documentElement;
    if(!root)return {error:"element not found"};
    var clone=root.cloneNode(true);
    if(removeScripts&&clone.querySelectorAll){
      var scripts=clone.querySelectorAll("script");
      for(var i=0;i<scripts.length;i++)scripts[i].parentNode.removeChild(scripts[i]);
    }
    if(removeComments&&document.createTreeWalker){
      var walker=document.createTreeWalker(clone,NodeFilter.SHOW_COMMENT,null);
      var comments=[];
      while(walker.nextNode())comments.push(walker.currentNode);
      for(var j=0;j<comments.length;j++)comments[j].parentNode.removeChild(comments[j]);
    }
    var html=clone.innerHTML;
    var truncated=false;
    if(html.length>maxLength){html=html.slice(0,maxLength);truncated=true;}
    return {html:html,truncated:truncated};
  }catch(e){return {error:String(e)};}
})()
"##;

/// console-logs 过滤 JS(占位符由 Rust 侧替换;返回 {logs:[...]} 已格式化为 "[level] msg")
/// 页面缓冲为 window.__btLogs = [{level,msg}],level ∈ log/info/warn/error/debug
const CONSOLE_LOGS_JS: &str = r##"
(function(){
  var all=window.__btLogs||[];
  var type=__BT_TYPE__;
  var search=__BT_SEARCH__;
  var limit=__BT_LIMIT__;
  var clear=__BT_CLEAR__;
  var out=[];
  for(var i=0;i<all.length;i++){
    var e=all[i]||{};
    if(type!=="all"&&String(e.level||"")!==type)continue;
    if(search&&String(e.msg||"").indexOf(search)<0)continue;
    out.push(e);
  }
  if(limit<out.length)out=out.slice(out.length-limit);
  if(clear)window.__btLogs=[];
  var lines=[];
  for(var j=0;j<out.length;j++)lines.push("["+(out[j].level||"log")+"] "+(out[j].msg||""));
  return {logs:lines};
})()
"##;

/// 读取整型参数:兼容 JSON 数字与命令行字符串(bt_cli 的 flag 默认 string 类型)
fn num_i64(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

/// 读取无符号整型参数:兼容 JSON 数字与命令行字符串
fn num_u64(body: &Value, key: &str) -> Option<u64> {
    body.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    })
}

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
            "/api/notify" => self.notify(&body).await,
            "/api/user-agent" => self.set_user_agent(&body).await,
            "/api/panel-config" => Ok(json!({ "ok": true })),
            "/api/close-session" => Ok(json!({ "closed": true })),
            "/api/accessibility" => self.accessibility(&body).await,
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
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let handle = self.handle.clone();
        // 可信点击:Windows 走 CDP 鼠标序列(命中测试 + 真实默认行为),非 Windows 明确报错
        let p = tokio::task::block_in_place(|| control::mouse::click(&handle, &s))?;
        Ok(json!({ "clicked": true, "x": p.x, "y": p.y }))
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
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let handle = self.handle.clone();
        // 可信悬停:Windows 走 CDP mouseMoved(真实移动触发 CSS :hover),非 Windows 明确报错
        let p = tokio::task::block_in_place(|| control::mouse::hover(&handle, &s))?;
        Ok(json!({ "hovered": true, "x": p.x, "y": p.y }))
    }

    async fn press_key(&self, body: &Value) -> Result<Value, String> {
        let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if key.is_empty() {
            return Err("key is required".into());
        }
        let selector = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        // 可信键盘注入:Windows 走 CDP(先聚焦 selector),非 Windows 未实现则明确报错
        tokio::task::block_in_place(|| control::keyboard::press_key(&handle, &key, &selector))?;
        Ok(json!({ "pressed": key }))
    }

    async fn drag(&self, body: &Value) -> Result<Value, String> {
        let from = body.get("sourceSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if from.is_empty() {
            return Err("sourceSelector is required".into());
        }
        let to = body.get("targetSelector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if to.is_empty() {
            return Err("targetSelector is required".into());
        }
        let handle = self.handle.clone();
        // 原生 DnD:Windows 走 CDP 鼠标序列(真实发起 dragstart/dragover/drop);非 Windows 明确报错
        tokio::task::block_in_place(|| control::drag::drag(&handle, &from, &to))?;
        Ok(json!({ "dragged": true }))
    }

    async fn upload_file(&self, body: &Value) -> Result<Value, String> {
        let s = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let path = body.get("filePath").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if path.is_empty() {
            return Err("filePath is required".into());
        }
        let handle = self.handle.clone();
        // 可信上传:Windows 走 CDP DOM.setFileInputFiles 并回读校验;非 Windows 明确报错
        tokio::task::block_in_place(|| control::upload::set_file(&handle, &s, &path))?;
        Ok(json!({ "uploaded": true }))
    }

    /// 截图:有 selector 时截该元素区域(视口 CSS 像素),否则整视口(行为不变)
    async fn screenshot(&self, body: &Value) -> Result<Value, String> {
        let selector = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let base64 = tokio::task::block_in_place(|| -> Result<String, String> {
            if selector.is_empty() {
                return crate::control::screenshot::screenshot(&handle);
            }
            // 元素矩形(视口 CSS 像素)由 element_state 提供
            let st = crate::control::element_state(&handle, &selector)?;
            if !st.get("exists").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(format!("element not found: {selector}"));
            }
            let rect = st.get("rect").ok_or("element rect missing")?;
            let x = rect.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            let y = rect.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            let w = rect.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            let h = rect.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            if w <= 0 || h <= 0 {
                return Err(format!("element has zero size: {selector}"));
            }
            crate::control::screenshot::screenshot_clip(&handle, x, y, w, h)
        })?;
        Ok(json!({ "base64": base64, "mime": "image/png" }))
    }

    async fn evaluate(&self, body: &Value) -> Result<Value, String> {
        let script = body.get("script").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::evaluate(&handle, &script))?;
        Ok(r)
    }

    /// 可见文本:有 selector 取该元素 innerText,否则取 body 全文
    async fn visible_text(&self, body: &Value) -> Result<Value, String> {
        let selector = body.get("selector").and_then(|v| v.as_str()).map(|s| s.to_string());
        let handle = self.handle.clone();
        let t = tokio::task::block_in_place(|| control::visible_text(&handle, selector.as_deref()))?;
        Ok(json!({ "text": t }))
    }

    /// 可见 HTML:支持 selector / removeScripts / removeComments / maxLength
    /// 默认 documentElement、移除 script、保留注释、截断 20000 字符
    async fn visible_html(&self, body: &Value) -> Result<Value, String> {
        let selector = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let remove_scripts = body
            .get("removeScripts")
            .and_then(|v| v.as_bool())
            .unwrap_or(DEFAULT_REMOVE_SCRIPTS);
        let remove_comments = body
            .get("removeComments")
            .and_then(|v| v.as_bool())
            .unwrap_or(DEFAULT_REMOVE_COMMENTS);
        let max_length = num_u64(body, "maxLength").unwrap_or(DEFAULT_HTML_MAX_LENGTH);
        // 占位符替换为 JS 字面量(selector 走 JSON 转义)
        let sel_lit = if selector.is_empty() {
            "null".to_string()
        } else {
            serde_json::to_string(&selector).unwrap_or_else(|_| "null".to_string())
        };
        let js = VISIBLE_HTML_JS
            .replace("__BT_SELECTOR__", &sel_lit)
            .replace("__BT_REMOVE_SCRIPTS__", if remove_scripts { "true" } else { "false" })
            .replace("__BT_REMOVE_COMMENTS__", if remove_comments { "true" } else { "false" })
            .replace("__BT_MAX_LENGTH__", &max_length.to_string());
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        if let Some(err) = r.get("error").and_then(|e| e.as_str()) {
            return Err(err.to_string());
        }
        Ok(json!({
            "html": r.get("html").and_then(|v| v.as_str()).unwrap_or(""),
            "truncated": r.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false),
        }))
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

    /// 滚动页面:同时兼容 {dx,dy} 精确偏移与 {direction,amount} 语义滚动
    /// 两者都缺省时按 amount=300 向下滚动(与 bt_cli `scroll` 文档一致)
    async fn scroll(&self, body: &Value) -> Result<Value, String> {
        let (dx, dy) = if body.get("dx").is_some() || body.get("dy").is_some() {
            // 显式偏移:直接使用
            (
                num_i64(body, "dx").unwrap_or(0) as i32,
                num_i64(body, "dy").unwrap_or(0) as i32,
            )
        } else {
            let direction = body.get("direction").and_then(|v| v.as_str()).unwrap_or("");
            let direction = if direction.is_empty() { DEFAULT_SCROLL_DIRECTION } else { direction };
            let amount = num_i64(body, "amount").unwrap_or(DEFAULT_SCROLL_AMOUNT) as i32;
            match direction {
                "down" => (0, amount),
                "up" => (0, -amount),
                "right" => (amount, 0),
                "left" => (-amount, 0),
                other => {
                    return Err(format!("unknown direction: {other} (up/down/left/right)"));
                }
            }
        };
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

    /// 读取页面控制台日志(由页面桥 __btLogs 捕获,元素形如 {level,msg})
    /// 支持 type(按 level 过滤:all/error/warning/log/info/debug,默认 all)、
    /// search(文本子串)、limit(取最后 N 条,默认 50)、clear(返回后清空缓冲)
    async fn console_logs(&self, body: &Value) -> Result<Value, String> {
        let type_raw = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .to_ascii_lowercase();
        // 页面存储的 level 为 warn(非 warning),此处统一归一
        let log_type = match type_raw.as_str() {
            "" | "all" => "all",
            "error" => "error",
            "warning" | "warn" => "warn",
            "log" => "log",
            "info" => "info",
            "debug" => "debug",
            other => {
                return Err(format!("unknown type: {other} (all/error/warning/log/info/debug)"));
            }
        };
        let search = body.get("search").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let limit = num_u64(body, "limit").unwrap_or(DEFAULT_CONSOLE_LOG_LIMIT);
        let clear = body.get("clear").and_then(|v| v.as_bool()).unwrap_or(false);
        let js = CONSOLE_LOGS_JS
            .replace("__BT_TYPE__", &serde_json::to_string(log_type).unwrap_or_else(|_| "\"all\"".into()))
            .replace("__BT_SEARCH__", &serde_json::to_string(&search).unwrap_or_else(|_| "\"\"".into()))
            .replace("__BT_LIMIT__", &limit.to_string())
            .replace("__BT_CLEAR__", if clear { "true" } else { "false" });
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::eval(&handle, &js))?;
        Ok(r)
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

    /// 可访问性树:有 selector 返回该元素子树,否则整页(行为不变)
    async fn accessibility(&self, body: &Value) -> Result<Value, String> {
        let selector = body.get("selector").and_then(|v| v.as_str()).map(|s| s.to_string());
        let handle = self.handle.clone();
        let tree = tokio::task::block_in_place(|| {
            crate::control::accessibility::accessibility_tree(&handle, selector.as_deref())
        })?;
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
        if iframe.is_empty() {
            return Err("iframeSelector is required".into());
        }
        let sel = body.get("selector").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if sel.is_empty() {
            return Err("selector is required".into());
        }
        let handle = self.handle.clone();
        // 可信 iframe 点击:iframe 内容坐标换算到顶层视口后走 CDP 鼠标序列
        let p = tokio::task::block_in_place(|| {
            control::mouse::click_in_iframe(&handle, &iframe, &sel)
        })?;
        Ok(json!({ "clicked": true, "x": p.x, "y": p.y }))
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
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let handle = self.handle.clone();
        // 可信点击(与 click 同通道),点击可能触发导航 → 读取当前 URL
        let p = tokio::task::block_in_place(|| control::mouse::click(&handle, &s))?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let handle2 = self.handle.clone();
        let url = tokio::task::block_in_place(|| {
            let v = control::page_state(&handle2).unwrap_or_default();
            v["url"].as_str().unwrap_or("").to_string()
        });
        Ok(json!({ "clicked": true, "url": url, "x": p.x, "y": p.y }))
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
    /// Linux(webkitgtk):wry 的 is_devtools_open 返回真实状态,用户在 devtools 窗口里点 ×
    /// 关闭后标记不会失同步,动作以真实状态为准,动作结果写回 UiState.devtools_open。
    /// Windows(webview2):wry 的 is_devtools_open 恒 false、close_devtools 为空实现,
    /// 无法探测/关闭,沿用 UiState 本地标记(close 仅标记状态,窗口需手动关闭)。
    async fn devtools(&self, body: &Value) -> Result<Value, String> {
        let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("toggle").to_string();
        let state = self.handle.state::<crate::ui::UiState>();
        let mut open = state.devtools_open.lock().unwrap();
        let handle = self.handle.clone();
        let now_open = tokio::task::block_in_place(|| {
            let page = ui::active_page_webview(&handle).ok_or("page webview not ready")?;
            // Linux 用 webview 真实状态;其他平台探测不到,退回本地标记
            #[cfg(target_os = "linux")]
            let actual = page.is_devtools_open();
            #[cfg(not(target_os = "linux"))]
            let actual = *open;

            let open_now = match action.as_str() {
                "open" => {
                    if !actual {
                        page.open_devtools();
                    }
                    true
                }
                "close" => {
                    if actual {
                        page.close_devtools();
                    }
                    false
                }
                "toggle" => {
                    if actual {
                        page.close_devtools();
                        false
                    } else {
                        page.open_devtools();
                        true
                    }
                }
                other => return Err(format!("unknown action: {other} (open/close/toggle)")),
            };
            Ok::<bool, String>(open_now)
        })?;
        // 动作后的真实结果写回本地标记,保持返回结构 { "open": bool }
        *open = now_open;
        Ok(json!({ "open": now_open }))
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
