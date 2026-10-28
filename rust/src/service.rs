//! 应用核心:HTTP 端点分发(基于 Tauri Webview 控制层)
//! 所有操作通过 control 模块 eval 驱动页面 Webview
use std::net::IpAddr;

use serde_json::{Value, json};
use tauri::AppHandle;
use tauri::Manager;

use crate::control;
use crate::streams;
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
/// console/network 查询默认返回条数(契约:`limit?:30`)
const DEFAULT_CONSOLE_LOG_LIMIT: u64 = 30;

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

/// 读取整型参数:兼容 JSON 数字与命令行字符串(bt_cli 的 flag 默认 string 类型)
fn num_i64(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    })
}

/// 读取布尔参数:兼容 JSON bool 与命令行字符串("true"/"1"/"false"/"0")
fn bool_arg(body: &Value, key: &str) -> Option<bool> {
    body.get(key).and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|n| n != 0),
        _ => None,
    })
}

/// macOS 未接入 CDP/WebKit 资源采集:相关端点明确报不支持,而不是静默返回空数组
fn unsupported_platform() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("console/network/expect is not supported on macOS")
    } else {
        None
    }
}

/// 解析 `--phase` 过滤参数(非法值明确报错;缺省为空=不过滤)
fn phase_arg(body: &Value) -> Result<String, String> {
    match body.get("phase") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => streams::parse_phase_filter(s),
        Some(other) => Err(format!(
            "phase must be a string (load|interactive|action:<cmd>), got: {other}"
        )),
    }
}

/// 把 0 基标签索引映射为内部 tab id(与 `list_tabs` 输出顺序一致;越界报错)
fn tab_id_for_index(ids: &[u32], idx: u64) -> Result<u32, String> {
    usize::try_from(idx)
        .ok()
        .and_then(|i| ids.get(i).copied())
        .ok_or_else(|| {
            format!(
                "tab index out of range: {idx} (0-based; {} tabs open)",
                ids.len()
            )
        })
}

/// `revealSecrets` 单独使用没有意义(不返回 headers):明确提示需配合 `--include-headers`
fn validate_reveal(include_headers: bool, reveal_secrets: bool) -> Result<(), String> {
    if reveal_secrets && !include_headers {
        return Err("revealSecrets requires includeHeaders (use --include-headers)".into());
    }
    Ok(())
}

/// 工具命令 → 动作相位名(1.12.0)。只列会驱动页面/网络的命令;
/// 查询类(visible-text/screenshot 等)不参与归因,避免噪声。
fn action_for_path(url: &str) -> Option<&'static str> {
    Some(match url {
        "/api/click" => "click",
        "/api/fill" => "fill",
        "/api/clear" => "clear",
        "/api/select" => "select",
        "/api/hover" => "hover",
        "/api/press-key" => "press_key",
        "/api/type" => "type",
        "/api/drag" => "drag",
        "/api/upload-file" => "upload_file",
        "/api/navigate" => "navigate",
        "/api/go-back" => "go_back",
        "/api/go-forward" => "go_forward",
        "/api/reload" => "reload",
        "/api/scroll" => "scroll",
        "/api/scroll-to-element" => "scroll_to_element",
        "/api/iframe-click" => "iframe_click",
        "/api/iframe-fill" => "iframe_fill",
        "/api/tabs/new" => "tabs_new",
        "/api/tabs/switch" => "tabs_switch",
        "/api/tabs/close" => "tabs_close",
        "/api/click-switch-tab" => "click_switch_tab",
        _ => return None,
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

/// `--timeout` 的合法区间(毫秒,与工具层钳制同口径):过小几乎必然误判,过大拖垮会话
const TIMEOUT_MIN_MS: u64 = 100;
const TIMEOUT_MAX_MS: u64 = 60_000;

/// 解析模型传入的 `--timeout`(毫秒),与既有 `wait_for_selector` 同单位
///
/// 返回 `(timeout_ms, clamped)`:
/// - **未传** → `(None, false)`:各原语走自己的默认(与 1.8.x 完全一致)
/// - 传了 → 必须是正整数,否则 Err(快速明确报错,不静默忽略);钳到 `[100, 60000]` 并标 `clamped`
fn timeout_arg(body: &Value) -> Result<(Option<u64>, bool), String> {
    match body.get("timeout") {
        None | Some(Value::Null) => Ok((None, false)),
        Some(v) => {
            let n = num_u64(body, "timeout")
                .ok_or_else(|| format!("timeout must be a positive integer (ms), got: {v}"))?;
            if n == 0 {
                return Err("timeout must be a positive integer (ms), got: 0".into());
            }
            let c = n.clamp(TIMEOUT_MIN_MS, TIMEOUT_MAX_MS);
            Ok((Some(c), c != n))
        }
    }
}

/// 把 `--timeout` 的**生效值**与**是否被钳制**写入响应体(仅模型显式传了才写,便于工具层标注)
fn with_timeout(mut out: Value, t: Option<u64>, clamped: bool) -> Value {
    if let Some(ms) = t {
        out["timeoutMs"] = json!(ms);
        out["timeoutClamped"] = json!(clamped);
    }
    out
}

/// 把降级标记写入结果 JSON(可信路径不写字段);`degraded`/`degradedReason` 与插件约定一致
fn mark_degraded(mut v: Value, degraded: &Option<String>) -> Value {
    if let Some(reason) = degraded {
        v["degraded"] = json!(true);
        v["degradedReason"] = json!(reason);
    }
    v
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
        Self {
            handle,
            browsers_path,
        }
    }

    /// 分发端点(client_ip 用于 `/api/console-log` 的 IP+tab 限速)
    pub async fn handle(
        &self,
        url: &str,
        body: Value,
        client_ip: Option<IpAddr>,
    ) -> Result<Value, String> {
        // 选择器统一入口校验:Playwright 风格(text= / xpath= / 以 / 开头等)在此快速明确报错,
        // 覆盖 click/fill/iframe_click 等**所有**收选择器字段的端点,不进点击/解析链路
        control::validate_selector_fields(&body)?;
        // 动作归因(1.12.0):工具命令执行期间相位 = `action:<cmd>`,
        // 结束后保留 3s 宽限;并发命令取最新(begin_action 覆盖旧窗口,不叠加)。
        let action = action_for_path(url)
            .map(|cmd| streams::begin_action(ui::active_tab_id(&self.handle), cmd));
        let result = self.dispatch(url, body, client_ip).await;
        if let Some(h) = action {
            streams::end_action(h);
        }
        result
    }

    /// 端点分发本体(不含动作相位包装)
    async fn dispatch(
        &self,
        url: &str,
        body: Value,
        client_ip: Option<IpAddr>,
    ) -> Result<Value, String> {
        match url {
            "/api/status" => self.status().await,
            "/api/title" => self.title().await,
            "/api/tabs" => self.tabs().await,
            "/api/navigate" => self.navigate(&body).await,
            "/api/close" => self.close().await,
            "/api/click" => self.click(&body).await,
            "/api/fill" => self.fill(&body).await,
            "/api/clear" => self.clear(&body).await,
            "/api/select" => self.select(&body).await,
            "/api/hover" => self.hover(&body).await,
            "/api/press-key" => self.press_key(&body).await,
            "/api/type" => self.type_text(&body).await,
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
            // 页面事件上报(SPA 路由/标题/图标;注入脚本 POST)
            "/api/page-event" => self.page_event(&body).await,
            "/api/go-back" => self.go_back().await,
            "/api/go-forward" => self.go_forward().await,
            "/api/resize" => self.resize(&body).await,
            "/api/console-logs" => self.console_logs(&body).await,
            // Linux 注入脚本回传(与 page-event 同级豁免:无令牌、放行 Origin,只写缓冲)
            "/api/console-log" => self.console_log_upload(&body, client_ip).await,
            "/api/network-logs" => self.network_logs(&body).await,
            "/api/expect-response" => self.expect_response(&body).await,
            "/api/assert-response" => self.assert_response(&body).await,
            "/api/notify" => self.notify(&body).await,
            "/api/user-agent" => self.set_user_agent(&body).await,
            "/api/panel-config" => Ok(json!({ "ok": true })),
            "/api/close-session" => {
                // 关闭会话:释放全部 tab 缓冲/相位/限速窗口
                streams::clear_all();
                Ok(json!({ "closed": true }))
            }
            "/api/accessibility" => self.accessibility(&body).await,
            "/api/iframe-click" => self.iframe_click(&body).await,
            "/api/iframe-fill" => self.iframe_fill(&body).await,
            "/api/tabs/new" => self.tabs_new(&body).await,
            "/api/tabs/switch" => self.tabs_switch(&body).await,
            "/api/tabs/close" => self.tabs_close(&body).await,
            "/api/click-switch-tab" => self.click_switch_tab(&body).await,
            // 面板相关(批注状态机)
            "/api/annotate/toggle" => self.annotate_toggle().await,
            "/api/annotate/records" => self.annotate_records(&body).await,
            "/api/annotate/send" => self.annotate_send().await,
            "/api/annotate/consume-sent" => self.annotate_consume_sent().await,
            // 推送式投递:插件注册通知地址(附着模式 / 晚注册)
            "/api/notify-url" => self.set_notify_url(&body).await,
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
            // 持久注入规则(1.12.0):规则 CRUD 与「只匹配不执行」自检
            "/api/inject/rules" => self.inject_rules(&body).await,
            "/api/inject/test" => self.inject_test(&body).await,
            // 下载列表与下载目录(1.13.0;同步操作,不驱动页面)
            "/api/downloads" => crate::downloads::api_downloads(&self.handle, &body),
            "/api/download-dir" => crate::downloads::api_download_dir(&body),
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
        let tabs_count = self
            .handle
            .state::<crate::ui::UiState>()
            .tabs
            .lock()
            .unwrap()
            .len();
        Ok(
            json!({ "open": true, "url": st.0, "title": st.1, "tabs": tabs_count, "installing": {} }),
        )
    }

    /// 返回主窗口标题(多会话实例自验用;标题来自 `--title`,默认 "bt-shell")
    async fn title(&self) -> Result<Value, String> {
        use tauri::Manager;
        let win = self
            .handle
            .get_window("main")
            .ok_or("main window not found")?;
        Ok(json!({ "title": win.title().map_err(|e| e.to_string())? }))
    }

    async fn navigate(&self, body: &Value) -> Result<Value, String> {
        let url = body
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            return Err("url is required".into());
        }
        // 裸域名自动补 http://
        let url = ui::normalize_url(&url);
        let handle = self.handle.clone();
        // 记录导航前的文档身份(URL + timeOrigin):用于确认导航真的发生(覆盖 SPA/同 URL 重载/bfcache)
        let prev = tokio::task::block_in_place(|| control::document_state(&handle).ok());
        let prev_url = prev
            .as_ref()
            .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(str::to_string))
            .unwrap_or_default();
        let prev_to = prev
            .as_ref()
            .and_then(|v| v.get("timeOrigin").and_then(|t| t.as_f64()));
        tokio::task::block_in_place(|| control::navigate(&handle, &url))?;
        // 同步激活标签的 URL(工具栏显示一致)
        ui::sync_active_tab(&self.handle, &url, "");
        // 等待就绪(需先确认导航发生,readyState>=interactive);有界超时,超时如实返回 ready:false
        let st = tokio::task::block_in_place(|| {
            control::wait_ready(&handle, &prev_url, prev_to, control::nav_timeout_ms())
        })?;
        Ok(json!({
            "url": url,
            "ready": st["ready"],
            "readyState": st["readyState"],
            "navigated": st["navigated"],
            "timedOut": st["timedOut"],
            "waitedMs": st["waitedMs"],
        }))
    }

    /// 在页面显示通知气泡(通过覆盖层 Webview 的 __btOverlay.notify)
    async fn notify(&self, body: &Value) -> Result<Value, String> {
        let message = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ty = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("ok")
            .to_string();
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
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:CDP 鼠标序列(命中测试 + 真实默认行为);非 Windows:降级 el.click() 并标注
        let o = tokio::task::block_in_place(|| control::mouse::click_with(&handle, &s, t))?;
        Ok(with_timeout(
            mark_degraded(
                json!({ "clicked": true, "x": o.value.x, "y": o.value.y }),
                &o.degraded,
            ),
            t,
            tclamped,
        ))
    }

    async fn fill(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let v = body
            .get("value")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:focus + 全选 + Input.insertText;非 Windows:降级 DOM setter + input/change 并标注
        let o = tokio::task::block_in_place(|| control::input::fill_with(&handle, &s, &v, t))?;
        Ok(with_timeout(
            mark_degraded(json!({ "filled": true }), &o.degraded),
            t,
            tclamped,
        ))
    }

    async fn clear(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        let o = tokio::task::block_in_place(|| control::input::clear_with(&handle, &s, t))?;
        Ok(with_timeout(
            mark_degraded(json!({ "cleared": true }), &o.degraded),
            t,
            tclamped,
        ))
    }

    async fn select(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let v = body
            .get("value")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:focus + 方向键可信选择(单值 select);multiple/size>1 或其他平台降级并标注
        let o = tokio::task::block_in_place(|| control::input::select_with(&handle, &s, &v, t))?;
        Ok(with_timeout(
            mark_degraded(json!({ "selected": true, "value": o.value }), &o.degraded),
            t,
            tclamped,
        ))
    }

    async fn hover(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:CDP mouseMoved(真实 CSS :hover);非 Windows:降级合成 mouseover/mouseenter 并标注
        let o = tokio::task::block_in_place(|| control::mouse::hover_with(&handle, &s, t))?;
        Ok(with_timeout(
            mark_degraded(
                json!({ "hovered": true, "x": o.value.x, "y": o.value.y }),
                &o.degraded,
            ),
            t,
            tclamped,
        ))
    }

    async fn press_key(&self, body: &Value) -> Result<Value, String> {
        let key = body
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if key.is_empty() {
            return Err("key is required".into());
        }
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 可信键盘注入:Windows 走 CDP(先聚焦 selector),非 Windows 未实现则明确报错
        tokio::task::block_in_place(|| {
            control::keyboard::press_key_with(&handle, &key, &selector, t)
        })?;
        Ok(with_timeout(json!({ "pressed": key }), t, tclamped))
    }

    /// 向当前焦点插入文本(不清空):有 selector 先聚焦,`enter` 为真时插入后再回车
    ///
    /// 与 `fill` 的差别:`fill` 是「聚焦 → 全选 → insertText」的替换语义;`type` 不清空、
    /// 不选中,直接插到光标处(xterm / 终端 / contenteditable 等场景用)。
    /// 返回 `{inserted, focused, entered}`:`inserted` 为字符数(char 计),
    /// `focused` 表示本次是否显式聚焦了 selector(无 selector 时为 false,沿用当前焦点)。
    async fn type_text(&self, body: &Value) -> Result<Value, String> {
        let text = body
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let enter = bool_arg(body, "enter").unwrap_or(false);
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 文本校验(空/超 8 KiB)在 plan_type 内先行完成,非法入参不会留下聚焦副作用
        let o = tokio::task::block_in_place(|| {
            control::keyboard::type_text_with(&handle, &text, &selector, enter, t)
        })?;
        Ok(with_timeout(
            json!({ "inserted": o.inserted, "focused": o.focused, "entered": o.entered }),
            t,
            tclamped,
        ))
    }

    async fn drag(&self, body: &Value) -> Result<Value, String> {
        let from = body
            .get("sourceSelector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if from.is_empty() {
            return Err("sourceSelector is required".into());
        }
        let to = body
            .get("targetSelector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if to.is_empty() {
            return Err("targetSelector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 原生 DnD:Windows 走 CDP 鼠标序列(真实发起 dragstart/dragover/drop);非 Windows 明确报错
        tokio::task::block_in_place(|| control::drag::drag_with(&handle, &from, &to, t))?;
        Ok(with_timeout(json!({ "dragged": true }), t, tclamped))
    }

    async fn upload_file(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let path = body
            .get("filePath")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if path.is_empty() {
            return Err("filePath is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 可信上传:Windows 走 CDP DOM.setFileInputFiles 并回读校验;非 Windows 明确报错
        tokio::task::block_in_place(|| control::upload::set_file_with(&handle, &s, &path, t))?;
        Ok(with_timeout(json!({ "uploaded": true }), t, tclamped))
    }

    /// 截图:无 selector → 整视口(行为不变);有 selector → 统一元素截图流程:
    /// ① exists 校验 ② 滚动到可见并等稳定(复用 scroll_to_element) ③ 重测 rect
    /// ④ rect 完全在视口内 → 常规 clip(原路径);仍超出视口 → 越界路径(beyond,整块截全)
    /// 滚动 reached/stable 不吞错:如实写进返回(beyondViewport/reached/stable/…)
    async fn screenshot(&self, body: &Value) -> Result<Value, String> {
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        let (base64, meta) = tokio::task::block_in_place(|| -> Result<(String, Value), String> {
            if selector.is_empty() {
                // 无 selector:整视口截图,保持现状
                let b = match t {
                    Some(ms) => crate::control::screenshot::screenshot_with_timeout(&handle, ms)?,
                    None => crate::control::screenshot::screenshot(&handle)?,
                };
                return Ok((b, json!({})));
            }
            // ① 元素存在性校验(错误口径与旧版一致)
            let st = crate::control::element_state_with(&handle, &selector, t)?;
            if !st.get("exists").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(format!("element not found: {selector}"));
            }
            // ② 先滚动到可见并等滚动稳定(超时如实返回 reached/stable,不吞错)
            let scroll = crate::control::scroll_to_element_with(&handle, &selector, t)?;
            // ③ 滚动后重测 rect(视口 CSS 像素,含懒加载/布局变化后的真实位置)
            let st = crate::control::element_state_with(&handle, &selector, t)?;
            if !st.get("exists").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Err(format!("element not found: {selector}"));
            }
            let rect = st.get("rect").ok_or("element rect missing")?;
            let x = rect
                .get("x")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .round() as i32;
            let y = rect
                .get("y")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .round() as i32;
            let w = rect
                .get("w")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .round() as i32;
            let h = rect
                .get("h")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .round() as i32;
            if w <= 0 || h <= 0 {
                return Err(format!("element has zero size: {selector}"));
            }
            // ④ 判定 rect 是否完全落在视口内(需视口尺寸;越界与否决定截图路径)
            let vp = crate::control::eval_with(
                &handle,
                "(function(){return {w:window.innerWidth,h:window.innerHeight};})()",
                t,
            )?;
            let vw = vp.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            let vh = vp.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            // 完全在视口内 → 常规 clip;任一边超出 → 越界路径(元素比视口还大,整块截全)。
            // 两条路径都传**视口**坐标,截图层内部统一换算文档坐标(CDP clip 实测恒为文档坐标)
            let beyond = x < 0 || y < 0 || x + w > vw || y + h > vh;
            let base64 = match (beyond, t) {
                (true, Some(ms)) => {
                    crate::control::screenshot::screenshot_clip_beyond_with_timeout(
                        &handle, x, y, w, h, ms,
                    )?
                }
                (true, None) => {
                    crate::control::screenshot::screenshot_clip_beyond(&handle, x, y, w, h)?
                }
                (false, Some(ms)) => crate::control::screenshot::screenshot_clip_with_timeout(
                    &handle, x, y, w, h, ms,
                )?,
                (false, None) => crate::control::screenshot::screenshot_clip(&handle, x, y, w, h)?,
            };
            let meta = json!({
                "beyondViewport": beyond,
                "reached": scroll.get("reached").cloned().unwrap_or(Value::Bool(false)),
                "inViewport": scroll.get("inViewport").cloned().unwrap_or(Value::Bool(false)),
                "stable": scroll.get("stable").cloned().unwrap_or(Value::Bool(false)),
                "waitedMs": scroll.get("waitedMs").cloned().unwrap_or(Value::Null),
            });
            // 滚动未到达时的 reason 原样带出(与 scroll_to_element 返回口径一致)
            let mut meta = meta;
            if let Some(reason) = scroll.get("reason") {
                meta["reason"] = reason.clone();
            }
            Ok((base64, meta))
        })?;
        let mut out = json!({ "base64": base64, "mime": "image/png" });
        // 越界/滚动元信息(无 selector 时 meta 为空对象,保持返回形状稳定)
        if let Some(m) = meta.as_object() {
            for (k, v) in m {
                out[k] = v.clone();
            }
        }
        Ok(with_timeout(out, t, tclamped))
    }

    async fn evaluate(&self, body: &Value) -> Result<Value, String> {
        let script = body
            .get("script")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, _tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // eval 超时是最直接的「快失败」手段:默认仍 15s(批次4 统一),传了就用传入值(不写 timeoutMs 到返回体 ——
        // evaluate 的返回体就是脚本结果本身,不掺元信息)
        let r = tokio::task::block_in_place(|| match t {
            Some(ms) => control::evaluate_timeout(&handle, &script, ms),
            None => control::evaluate(&handle, &script),
        })?;
        Ok(r)
    }

    /// 持久注入规则 CRUD:`action=add|list|remove|clear|enable|disable`
    ///
    /// 文件位于 `<sessionDataDir>/inject/rules.json`;同步文件 IO 放进阻塞线程,
    /// 失败只返回可读原因(不含绝对路径)。
    async fn inject_rules(&self, body: &Value) -> Result<Value, String> {
        let dir = crate::inject::session_dir(&self.handle);
        tokio::task::block_in_place(|| crate::inject::rules_action(&dir, body))
    }

    /// 注入规则自检:只按 url 匹配,**不执行**任何脚本
    async fn inject_test(&self, body: &Value) -> Result<Value, String> {
        let url = body
            .get("url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if url.is_empty() {
            return Err("url is required".into());
        }
        let dir = crate::inject::session_dir(&self.handle);
        tokio::task::block_in_place(|| crate::inject::test_match(&dir, &url))
    }

    /// 可见文本:有 selector 取该元素 innerText,否则取 body 全文
    async fn visible_text(&self, body: &Value) -> Result<Value, String> {
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let handle = self.handle.clone();
        let t =
            tokio::task::block_in_place(|| control::visible_text(&handle, selector.as_deref()))?;
        Ok(json!({ "text": t }))
    }

    /// 可见 HTML:支持 selector / removeScripts / removeComments / maxLength
    /// 默认 documentElement、移除 script、保留注释、截断 20000 字符
    async fn visible_html(&self, body: &Value) -> Result<Value, String> {
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
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
            .replace(
                "__BT_REMOVE_SCRIPTS__",
                if remove_scripts { "true" } else { "false" },
            )
            .replace(
                "__BT_REMOVE_COMMENTS__",
                if remove_comments { "true" } else { "false" },
            )
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
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        let r = tokio::task::block_in_place(|| control::element_state_with(&handle, &s, t))?;
        Ok(with_timeout(r, t, tclamped))
    }

    async fn dropdown_options(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        let js = format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el || !el.options) return {{options:[]}};
              return {{options: Array.from(el.options).map(o => ({{value:o.value, text:o.text}}))}};
            }})()"#,
            s = s
        );
        let r = tokio::task::block_in_place(|| control::eval_with(&handle, &js, t))?;
        Ok(with_timeout(r, t, tclamped))
    }

    async fn wait_for_selector(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // 修:原 `as_u64()` 只认 JSON 数字,而 bt_cli 的 flag 一律是字符串 → `--timeout` 被静默忽略。
        // 改走 timeout_arg:兼容字符串、钳到 [100,60000]、未传保持默认 15000ms(批次4 与其它超时统一),并如实回显生效值
        let (t, tclamped) = timeout_arg(body)?;
        let timeout = t.unwrap_or(15_000);
        let handle = self.handle.clone();
        let found =
            tokio::task::block_in_place(|| control::wait_for_selector(&handle, &s, timeout))?;
        Ok(with_timeout(json!({ "found": found }), t, tclamped))
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
            let direction = if direction.is_empty() {
                DEFAULT_SCROLL_DIRECTION
            } else {
                direction
            };
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
        // 等待滚动稳定(平滑滚动异步);超时如实返回 stable:false
        let r = tokio::task::block_in_place(|| control::scroll(&handle, dx, dy))?;
        Ok(json!({
            "scrolled": true,
            "stable": r["stable"],
            "timedOut": r["timedOut"],
            "x": r["x"],
            "y": r["y"],
            "waitedMs": r["waitedMs"],
        }))
    }

    async fn scroll_to_element(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 等待元素真正进入视口且滚动稳定;超时如实返回 reached:false(+reason)
        let r = tokio::task::block_in_place(|| control::scroll_to_element_with(&handle, &s, t))?;
        let mut out = json!({
            "scrolled": true,
            "reached": r["reached"],
            "inViewport": r["inViewport"],
            "stable": r["stable"],
            "timedOut": r["timedOut"],
            "waitedMs": r["waitedMs"],
        });
        if r.get("reason").is_some() {
            out["reason"] = r["reason"].clone();
        }
        Ok(with_timeout(out, t, tclamped))
    }

    async fn reload(&self) -> Result<Value, String> {
        // 错误页 -> 重新导航原 URL;否则原生 reload(见 ui::reload_active_tab)
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| ui::reload_active_tab(&handle))
    }

    /// 页面事件上报(SPA 路由/标题/图标变化;注入脚本以 no-cors 简单 POST 到此。
    /// 1.11.0 / bt-shell 0.11.0 起 http.rs 只对面板自身 origin 放行 CORS(其它路径的非面板 Origin 403、
    /// 非面板 OPTIONS 405),此路径仍是唯一的「令牌 + Origin」双豁免(精确匹配))
    async fn page_event(&self, body: &Value) -> Result<Value, String> {
        let label = body.get("label").and_then(|v| v.as_str()).unwrap_or("");
        if label.is_empty() {
            return Ok(json!({ "ok": false, "reason": "missing label" }));
        }
        let url = body.get("url").and_then(|v| v.as_str());
        let title = body.get("title").and_then(|v| v.as_str());
        let icon = body.get("icon").and_then(|v| v.as_str());
        let changed = crate::ui::apply_page_state(&self.handle, label, url, title, icon);
        if changed {
            crate::ui::emit_tabs_changed(&self.handle);
        }
        Ok(json!({ "ok": true, "changed": changed }))
    }

    async fn go_back(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        // 记录历史导航前的文档身份;history.back 可能是 bfcache 恢复(timeOrigin 变化)或重载
        let prev = tokio::task::block_in_place(|| control::document_state(&handle).ok());
        let prev_url = prev
            .as_ref()
            .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(str::to_string))
            .unwrap_or_default();
        let prev_to = prev
            .as_ref()
            .and_then(|v| v.get("timeOrigin").and_then(|t| t.as_f64()));
        tokio::task::block_in_place(|| control::eval(&handle, "history.back(); true"))?;
        // 等待导航就绪(文档身份/URL 变化 + readyState);有界超时,超时如实返回 ready:false
        let st = tokio::task::block_in_place(|| {
            control::wait_ready(&handle, &prev_url, prev_to, control::history_timeout_ms())
        })?;
        Ok(json!({
            "back": true,
            "url": st["url"],
            "ready": st["ready"],
            "readyState": st["readyState"],
            "navigated": st["navigated"],
            "timedOut": st["timedOut"],
            "waitedMs": st["waitedMs"],
        }))
    }

    async fn go_forward(&self) -> Result<Value, String> {
        let handle = self.handle.clone();
        let prev = tokio::task::block_in_place(|| control::document_state(&handle).ok());
        let prev_url = prev
            .as_ref()
            .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(str::to_string))
            .unwrap_or_default();
        let prev_to = prev
            .as_ref()
            .and_then(|v| v.get("timeOrigin").and_then(|t| t.as_f64()));
        tokio::task::block_in_place(|| control::eval(&handle, "history.forward(); true"))?;
        let st = tokio::task::block_in_place(|| {
            control::wait_ready(&handle, &prev_url, prev_to, control::history_timeout_ms())
        })?;
        Ok(json!({
            "forward": true,
            "url": st["url"],
            "ready": st["ready"],
            "readyState": st["readyState"],
            "navigated": st["navigated"],
            "timedOut": st["timedOut"],
            "waitedMs": st["waitedMs"],
        }))
    }

    /// 调整页面内容区尺寸(视口语义):width/height 指内容区(视口),窗口高 = 内容高 + 工具栏 72px
    /// 内容区下限 360x360,低于下限被钳制并在返回里如实标 clamped(模型可见)
    /// 屏幕工作区防护:目标窗口超出工作区 → 钳到工作区(标 screenClamped);set_size 后立即主动重排,
    /// 不单独依赖系统 Resized 事件(避免极端时序下不重排 → 内容全黑)
    async fn resize(&self, body: &Value) -> Result<Value, String> {
        let w = body.get("width").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        let h = body.get("height").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
        // 内容区尺寸 clamp 到下限(逻辑像素)
        let cw = w.max(ui::CONTENT_MIN_SIZE);
        let ch = h.max(ui::CONTENT_MIN_SIZE);
        let clamped = cw != w || ch != h;
        let handle = self.handle.clone();
        tokio::task::block_in_place(|| {
            let win = ui::active_page_webview(&handle)
                .ok_or("page webview not ready")?
                .window_ref()
                .clone();
            // 目标窗口 = 内容区宽 x (内容区高 + 工具栏高)
            let want_w = cw;
            let want_h = ch + ui::TOOLBAR_HEIGHT;
            // 屏幕工作区防护:尺寸超出 → 钳到工作区;出屏 → 回位(保持窗口可见;helper 内部统一逻辑像素口径)
            let (x, y, fw, fh, screen_clamped) =
                ui::clamp_window_to_work_area(&win, want_w, want_h);
            // 先定位再设尺寸;位置与尺寸都是**逻辑**像素(set_position/set_size 口径一致,避免窗口漂移)
            win.set_position(tauri::LogicalPosition::new(x, y))
                .map_err(|e| format!("resize set position failed: {e}"))?;
            win.set_size(tauri::LogicalSize::new(fw, fh))
                .map_err(|e| format!("resize failed: {e}"))?;
            // 主动重排一次(带日志+重试):用**目标尺寸**(LogicalSize→物理)换算,不赌系统 Resized 事件。
            // 不读 inner_size():set_size 异步生效,立即读到的可能是旧值 → 会拿旧尺寸排版
            let scale = win.scale_factor().unwrap_or(1.0);
            let phys: tauri::PhysicalSize<u32> = tauri::LogicalSize::new(fw, fh).to_physical(scale);
            ui::apply_layout_or_log(&handle, phys, scale);
            // 钳制后的真实内容区 = 窗口高 - 工具栏
            let real_ch = (fh - ui::TOOLBAR_HEIGHT).max(ui::CONTENT_MIN_SIZE);
            let real_cw = fw.max(ui::CONTENT_MIN_SIZE);
            Ok(json!({
                "resized": true,
                "content": { "w": real_cw, "h": real_ch },
                "window": { "w": fw, "h": fh },
                "clamped": clamped,
                // 是否被屏幕工作区钳制(模型可见,与 clamped 的内容区下限钳制分开)
                "screenClamped": screen_clamped,
            }))
        })
    }

    /// 把请求里的 `tab`(0 基索引)映射为内部 tab id;未传返回 None(全部标签)
    fn resolve_tab(&self, body: &Value) -> Result<Option<u32>, String> {
        let Some(raw) = body.get("tab").filter(|v| !v.is_null()) else {
            return Ok(None);
        };
        let idx = num_i64(body, "tab").ok_or_else(|| {
            format!("tab must be a non-negative integer (0-based), got: {raw}")
        })?;
        if idx < 0 {
            return Err(format!(
                "tab index out of range: {idx} (0-based; must be >= 0)"
            ));
        }
        let state = self.handle.state::<crate::ui::UiState>();
        let tabs = state.tabs.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<u32> = tabs.iter().map(|t| t.id).collect();
        tab_id_for_index(&ids, idx as u64).map(Some)
    }

    /// 读取控制台流(1.12.0:读 Rust 侧缓冲,不再 eval 页面 `__btLogs`)
    ///
    /// 请求(向后兼容旧字段):
    /// `{limit?:30, type?:"all|log|info|warn|error|debug|exceptions", search?, group?:"burst|none",
    ///   phase?:"load|interactive|action:<cmd>", tab?:number(0 基索引), clear?:boolean}`
    ///
    /// 响应 `data`:`{items, logs, dropped, bytes, total, limit, type, search, group, phase, clear}`
    /// - `items` 为契约字段;`logs` 为旧的 `"[level] msg"` 文本数组(旧插件兼容)。
    async fn console_logs(&self, body: &Value) -> Result<Value, String> {
        if let Some(err) = unsupported_platform() {
            return Err(err.to_string());
        }
        let type_raw = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .to_ascii_lowercase();
        let log_type = match type_raw.as_str() {
            "" | "all" => "all",
            "error" => "error",
            "warning" | "warn" => "warn",
            "log" => "log",
            "info" => "info",
            "debug" => "debug",
            "exceptions" => "exceptions",
            other => {
                return Err(format!(
                    "unknown type: {other} (all/error/warning/log/info/debug/exceptions)"
                ));
            }
        };
        let group = match body
            .get("group")
            .and_then(|v| v.as_str())
            .unwrap_or("burst")
        {
            "" | "burst" => "burst",
            "none" => "none",
            other => return Err(format!("unknown group: {other} (burst/none)")),
        };
        let search = body
            .get("search")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let limit = num_u64(body, "limit").unwrap_or(DEFAULT_CONSOLE_LOG_LIMIT) as usize;
        let clear = bool_arg(body, "clear").unwrap_or(false);
        let tab = self.resolve_tab(body)?;
        let phase = phase_arg(body)?;
        // 浏览器内部日志(violation/deprecation)默认不返回
        let include_internal = bool_arg(body, "includeInternal").unwrap_or(false);
        let q = streams::ConsoleQuery {
            typ: log_type.to_string(),
            search,
            group: group.to_string(),
            phase,
            tab,
            include_internal,
        };
        Ok(streams::query_console(&q, limit, clear))
    }

    /// Linux 注入脚本回传控制台条目(页面可用端点)
    ///
    /// 请求 `{entries:[{level,msg,ts?,stack?}], tab?:number, label?:string}`。
    /// 只写缓冲、不执行任何内容;按 IP+tab 限速 200 条/秒,超限计入 `dropped`。
    /// body 大小由 http.rs 限制为 256 KiB。
    async fn console_log_upload(
        &self,
        body: &Value,
        client_ip: Option<IpAddr>,
    ) -> Result<Value, String> {
        if let Some(err) = unsupported_platform() {
            return Err(err.to_string());
        }
        let entries = body
            .get("entries")
            .and_then(|v| v.as_array())
            .ok_or("entries is required")?;
        // 归属标签:显式 tab 优先,其次页面注入的 webview label
        let tab = num_u64(body, "tab")
            .map(|t| t as u32)
            .or_else(|| {
                body.get("label")
                    .and_then(|v| v.as_str())
                    .and_then(|label| ui::tab_id_for_label(&self.handle, label))
            })
            .unwrap_or(0);
        // 单请求处理上限:防止恶意页面用 256 KiB 塞入海量条目长时间占锁
        const MAX_UPLOAD_ENTRIES: usize = 2_000;
        let total = entries.len();
        let ip = client_ip.unwrap_or(IpAddr::from([127, 0, 0, 1]));
        let (accepted, over_rate) =
            streams::rate_limit_console(ip, tab, total.min(MAX_UPLOAD_ENTRIES));
        let overflow = total.saturating_sub(MAX_UPLOAD_ENTRIES);
        let dropped = over_rate + overflow;
        streams::note_console_dropped(tab, dropped);
        for e in entries.iter().take(accepted) {
            let level = streams::normalize_console_level(
                e.get("level").and_then(|v| v.as_str()).unwrap_or("log"),
            );
            let msg = e
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ts = e
                .get("ts")
                .and_then(|v| v.as_u64())
                .filter(|t| *t > 0)
                .unwrap_or_else(streams::now_ms);
            let stack = e
                .get("stack")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            streams::push_console(streams::ConsoleEvent {
                level,
                msg,
                ts,
                phase: String::new(),
                source: streams::ConsoleSource::Injected,
                stack,
                tab,
                is_exception: false,
                internal: false,
            });
        }
        Ok(json!({ "ok": true, "accepted": accepted, "dropped": dropped }))
    }

    /// 读取网络流(1.12.0)
    ///
    /// 请求:`{limit?:30, type?:"all|xhr|fetch|doc|script|image|font|other",
    ///   status?:"2xx|3xx|4xx|5xx"|number, failed?:boolean, search?, group?:"burst|key|none",
    ///   phase?:"load|interactive|action:<cmd>", tab?:number(0 基索引),
    ///   includeHeaders?:boolean, revealSecrets?:boolean, clear?:boolean}`
    async fn network_logs(&self, body: &Value) -> Result<Value, String> {
        if let Some(err) = unsupported_platform() {
            return Err(err.to_string());
        }
        let typ_raw = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("all")
            .to_ascii_lowercase();
        let typ = match typ_raw.as_str() {
            "" | "all" => "all",
            "xhr" => "xhr",
            "fetch" => "fetch",
            "doc" | "document" => "doc",
            "script" => "script",
            "image" => "image",
            "font" => "font",
            "other" => "other",
            other => {
                return Err(format!(
                    "unknown type: {other} (all/xhr/fetch/doc/script/image/font/other)"
                ));
            }
        };
        let status = match body.get("status") {
            None | Some(Value::Null) => String::new(),
            Some(Value::Number(n)) => n.to_string(),
            Some(Value::String(s)) => s.trim().to_string(),
            Some(other) => return Err(format!("invalid status: {other}")),
        };
        // 非法 status(非 2xx/3xx/4xx/5xx/纯数字)直接报参数错误,不再静默返回全部
        streams::validate_status_filter(&status)?;
        let group = match body
            .get("group")
            .and_then(|v| v.as_str())
            .unwrap_or("burst")
        {
            "" | "burst" => "burst",
            "key" => "key",
            "none" => "none",
            other => return Err(format!("unknown group: {other} (burst/key/none)")),
        };
        let search = body
            .get("search")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let limit = num_u64(body, "limit").unwrap_or(DEFAULT_CONSOLE_LOG_LIMIT) as usize;
        let tab = self.resolve_tab(body)?;
        let phase = phase_arg(body)?;
        let include_headers = bool_arg(body, "includeHeaders").unwrap_or(false);
        let reveal_secrets = bool_arg(body, "revealSecrets").unwrap_or(false);
        validate_reveal(include_headers, reveal_secrets)?;
        let clear = bool_arg(body, "clear").unwrap_or(false);
        let q = streams::NetworkQuery {
            typ: typ.to_string(),
            status,
            failed: bool_arg(body, "failed"),
            search,
            group: group.to_string(),
            phase,
            tab,
        };
        Ok(streams::query_network(
            &q,
            limit,
            include_headers,
            reveal_secrets,
            clear,
        ))
    }

    /// 记录期望:只记匹配起点(**不清空** network 流,保留 `network_logs` 历史)
    async fn expect_response(&self, body: &Value) -> Result<Value, String> {
        if let Some(err) = unsupported_platform() {
            return Err(err.to_string());
        }
        let pattern = body
            .get("urlPattern")
            .or_else(|| body.get("url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let since = streams::mark_expect(&pattern);
        Ok(json!({ "expected": true, "pattern": pattern, "since": since }))
    }

    /// 断言响应:在 network 流中匹配 pattern(URL 子串),返回最近命中
    ///
    /// 若调用过 `expect_response`,只匹配起点之后的条目(历史仍保留在 `network_logs`)。
    /// 返回字段保持兼容:`{matched, url, status}` / `{matched:false, error}`
    async fn assert_response(&self, body: &Value) -> Result<Value, String> {
        if let Some(err) = unsupported_platform() {
            return Err(err.to_string());
        }
        let pattern = body
            .get("id")
            .or_else(|| body.get("urlPattern"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // 未显式传 pattern 时回退到 expect_response 记录的模式
        let pattern = if pattern.is_empty() {
            streams::expect_pattern().unwrap_or_default()
        } else {
            pattern
        };
        if pattern.is_empty() {
            return Ok(json!({ "matched": false, "error": "no pattern provided" }));
        }
        match streams::find_network(&pattern, streams::expect_cursor()) {
            Some((url, status)) => Ok(json!({ "matched": true, "url": url, "status": status })),
            None => Ok(json!({ "matched": false, "error": "no response matched the pattern yet" })),
        }
    }

    /// 运行时修改页面 User-Agent(Windows 通过 ICoreWebView2Settings2,其他平台报错)
    async fn set_user_agent(&self, body: &Value) -> Result<Value, String> {
        let ua = body
            .get("userAgent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if ua.is_empty() {
            return Err("userAgent is required".into());
        }
        let handle = self.handle.clone();
        // 恢复默认走 /api/device 的特殊名 __default__(此处仅覆盖,保持"非空"校验)
        tokio::task::block_in_place(|| crate::devices::set_user_agent(&handle, Some(&ua)))?;
        Ok(json!({ "set": true, "ua": ua }))
    }

    /// 可访问性树:有 selector 返回该元素子树,否则整页(行为不变)
    async fn accessibility(&self, body: &Value) -> Result<Value, String> {
        let selector = body
            .get("selector")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // 带 selector 是 4 步 CDP 链,每步都用传入预算(总最坏 ≈ 4 × 单步)
        let tree = tokio::task::block_in_place(|| {
            crate::control::accessibility::accessibility_tree_with(&handle, selector.as_deref(), t)
        })?;
        Ok(with_timeout(tree, t, tclamped))
    }

    /// 标签列表(真实多标签,含 id/url/title)
    async fn tabs(&self) -> Result<Value, String> {
        let state = self.handle.state::<crate::ui::UiState>();
        let tabs = state.tabs.lock().unwrap().clone();
        let list: Vec<serde_json::Value> = tabs
            .iter()
            .map(|t| json!({ "id": t.id, "url": t.url, "title": t.title, "icon": t.icon, "errored": t.errored }))
            .collect();
        Ok(json!({ "tabs": list }))
    }

    /// 新建标签:真多标签独立 Webview 并导航(工具栏/+ / target=_blank 共用逻辑)
    async fn tabs_new(&self, body: &Value) -> Result<Value, String> {
        let url = body
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
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
        let tid = tabs
            .get(idx)
            .map(|t| t.id)
            .ok_or("tab index out of range")?;
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
            Some(i) if i >= 0 => tabs
                .get(i as usize)
                .map(|t| t.id)
                .ok_or("tab index out of range")?,
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
        let iframe = body
            .get("iframeSelector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if iframe.is_empty() {
            return Err("iframeSelector is required".into());
        }
        let sel = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if sel.is_empty() {
            return Err("selector is required".into());
        }
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:iframe 内容坐标换算后走可信鼠标序列;非 Windows:降级同源 iframe el.click() 并标注
        let o = tokio::task::block_in_place(|| {
            control::mouse::click_in_iframe_with(&handle, &iframe, &sel, t)
        })?;
        Ok(with_timeout(
            mark_degraded(
                json!({ "clicked": true, "x": o.value.x, "y": o.value.y }),
                &o.degraded,
            ),
            t,
            tclamped,
        ))
    }

    /// 在 iframe 中填写输入框
    async fn iframe_fill(&self, body: &Value) -> Result<Value, String> {
        let iframe = body
            .get("iframeSelector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if iframe.is_empty() {
            return Err("iframeSelector is required".into());
        }
        let sel = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if sel.is_empty() {
            return Err("selector is required".into());
        }
        let val = body
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let (t, tclamped) = timeout_arg(body)?;
        let handle = self.handle.clone();
        // Windows:iframe 内 focus + insertText;非 Windows:降级 DOM setter + input/change 并标注
        let o = tokio::task::block_in_place(|| {
            control::input::fill_in_iframe_with(&handle, &iframe, &sel, &val, t)
        })?;
        Ok(with_timeout(
            mark_degraded(json!({ "filled": true }), &o.degraded),
            t,
            tclamped,
        ))
    }

    async fn click_switch_tab(&self, body: &Value) -> Result<Value, String> {
        let s = body
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if s.is_empty() {
            return Err("selector is required".into());
        }
        let handle = self.handle.clone();
        // 与 click 同通道(可信/降级一致),点击可能触发导航 → 读取当前 URL
        let o = tokio::task::block_in_place(|| control::mouse::click(&handle, &s))?;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let handle2 = self.handle.clone();
        let url = tokio::task::block_in_place(|| {
            let v = control::page_state(&handle2).unwrap_or_default();
            v["url"].as_str().unwrap_or("").to_string()
        });
        Ok(mark_degraded(
            json!({ "clicked": true, "url": url, "x": o.value.x, "y": o.value.y }),
            &o.degraded,
        ))
    }

    // ---- 批注状态机端点 ----

    /// 应用设备预设(内容区尺寸 + UA):预设表宽高即内容区(视口)尺寸,
    /// 窗口 = 内容区高 + 工具栏 TOOLBAR_HEIGHT
    /// 特殊名 `__default__`(devices::DEFAULT_DEVICE_NAME,面板首项「默认」)→ 恢复默认 UA + 内容区回 1100x628
    async fn device(&self, body: &Value) -> Result<Value, String> {
        let name = body
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let device =
            crate::devices::find(&name).ok_or_else(|| format!("unknown device: {name}"))?;
        let handle = self.handle.clone();
        // apply 内 UA 必被处理(覆盖或恢复),失败直接报错不静默 → ua=(default) 只在真重置后才可能返回
        // 返回**实际生效**内容区(大预设被工作区钳制时 ≠ 预设值),避免返回体与真实窗口不一致
        let (cw, ch, screen_clamped) =
            tokio::task::block_in_place(|| crate::devices::apply(&handle, device))?;
        Ok(json!({
            "device": device.name,
            // width/height 保持字段名不变,语义改为**实际**内容区(视口)尺寸(未被钳制时 = 预设值)
            "width": cw,
            "height": ch,
            "content": { "w": cw, "h": ch },
            "window": { "w": cw, "h": ch + ui::TOOLBAR_HEIGHT },
            "ua": device.ua.unwrap_or("(default)"),
            // 本次是否把 UA 恢复成默认(桌面预设/「默认」项为 true;覆盖型预设为 false)
            "uaReset": device.ua.is_none(),
            // 是否被屏幕工作区钳制(与 resize 的同名字段一致)
            "screenClamped": screen_clamped,
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
    ///
    /// Linux(webkitgtk):wry 的 is_devtools_open 返回真实状态,用户在 devtools 窗口里点 ×
    /// 关闭后标记不会失同步,动作以真实状态为准,结果写回 UiState.devtools_open。
    /// Windows(webview2):wry 0.55.1 的 `close_devtools()` 是**空实现**
    /// (见 wry-0.55.1/src/webview2/mod.rs:`pub fn close_devtools(&self) {}`),
    /// 且 WebView2 接口只有 `OpenDevToolsWindow`,没有对应的 Close API —— 无法真正关闭。
    /// 因此 close/toggle-close 不再谎报 `open:false`,而是如实返回 `{open:true, supported:false}` 并留日志。
    /// 返回体固定带 `open`(真实/推定状态)与 `supported`(该平台是否支持真正关闭)。
    async fn devtools(&self, body: &Value) -> Result<Value, String> {
        let action = body
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("toggle")
            .to_string();
        let state = self.handle.state::<crate::ui::UiState>();
        let mut open = state.devtools_open.lock().unwrap();
        let handle = self.handle.clone();
        // 能否真正关闭 devtools:Linux 由 webkitgtk 实现;Windows 侧 wry 为空实现
        let close_supported = cfg!(target_os = "linux");
        let (now_open, closed) = tokio::task::block_in_place(|| {
            let page = ui::active_page_webview(&handle).ok_or("page webview not ready")?;
            // Linux 用 webview 真实状态;其他平台探测不到,退回本地标记
            #[cfg(target_os = "linux")]
            let actual = page.is_devtools_open();
            #[cfg(not(target_os = "linux"))]
            let actual = *open;

            let state_now = match action.as_str() {
                "open" => {
                    if !actual {
                        page.open_devtools();
                    }
                    (true, true)
                }
                "close" => {
                    if !close_supported {
                        // 无法真正关闭:保持 open=true,不让调用方误以为已关
                        eprintln!(
                            "[devtools] close unsupported on this platform (wry close_devtools is a no-op, WebView2 has no close API); devtools stays open"
                        );
                        (true, false)
                    } else {
                        if actual {
                            page.close_devtools();
                        }
                        (false, true)
                    }
                }
                "toggle" => {
                    if actual {
                        if !close_supported {
                            eprintln!(
                                "[devtools] toggle->close unsupported on this platform (wry close_devtools is a no-op); devtools stays open"
                            );
                            (true, false)
                        } else {
                            page.close_devtools();
                            (false, true)
                        }
                    } else {
                        page.open_devtools();
                        (true, true)
                    }
                }
                other => return Err(format!("unknown action: {other} (open/close/toggle)")),
            };
            Ok::<(bool, bool), String>(state_now)
        })?;
        // 动作后的状态写回本地标记,保持 `{ "open": bool }` 既有语义(新增 supported 为附加字段)
        *open = now_open;
        Ok(json!({ "open": now_open, "supported": closed }))
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
        let kind = body
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
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
                let digits = body
                    .get("digits")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                format!("(window.__btFakeMic?window.__btFakeMic.dtmf({digits:?}):0)")
            }
            "noise" => {
                let dur = body.get("durMs").and_then(|v| v.as_f64()).unwrap_or(400.0);
                format!("(window.__btFakeMic?window.__btFakeMic.noise({dur}):0)")
            }
            "audio" => {
                let data = body
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
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
                return Err(format!(
                    "unknown kind: {other} (tone/seq/dtmf/noise/audio/ambient/stop)"
                ));
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
        let kind = body
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let js = match kind.as_str() {
            "image" => {
                let data = body
                    .get("data")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if data.is_empty() {
                    return Err("data (base64 image) is required for kind=image".into());
                }
                format!("(window.__btFakeCam?window.__btFakeCam.setImage({data:?}):0)")
            }
            "video" => {
                let url = body
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
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

    /// 批注/截图记录列表
    ///
    /// body.includeImages(默认 **false**):true 时每条附带 `image`(PNG base64);
    /// false 时只给 `hasImage` 标记不带 base64 —— 列表类调用(面板/`list_records`)无需整批图片,
    /// 只有确需内容的 `read_record_content` 才显式要图,避免几十条截图 base64 一次性塞进响应。
    async fn annotate_records(&self, body: &Value) -> Result<Value, String> {
        let include_images = body
            .get("includeImages")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let state = self.handle.state::<crate::ui::UiState>();
        let records = state.records.lock().unwrap().clone();
        let list: Vec<Value> = records
            .iter()
            .map(|r| {
                let mut obj = serde_json::Map::new();
                obj.insert("index".into(), json!(r.index));
                obj.insert("type".into(), json!(r.typ));
                obj.insert("url".into(), json!(r.url));
                obj.insert("selector".into(), json!(r.selector));
                obj.insert(
                    "rect".into(),
                    json!([r.rect.0, r.rect.1, r.rect.2, r.rect.3]),
                );
                obj.insert("note".into(), json!(r.note));
                obj.insert("sent".into(), json!(r.sent));
                // 每条都给 hasImage:调用方据此决定是否值得再按需取图
                obj.insert("hasImage".into(), json!(r.image.is_some()));
                // 仅显式要图时带上 base64(默认不带,避免整批大图塞进响应)
                if include_images && let Some(img) = &r.image {
                    obj.insert("image".into(), json!(img));
                }
                Value::Object(obj)
            })
            .collect();
        Ok(json!(list))
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
        // 诊断日志("面板发送 → agent 收到"链路第 3 断点;经插件转发写入日志文件)。
        // 降噪:仅在非空时记录(轮询每 2s 一次,空结果不打日志)
        if !items.is_empty() {
            eprintln!(
                "[send] consume-sent: {} record(s) ids={:?}",
                items.len(),
                items
                    .iter()
                    .filter_map(|v| v.get("index").and_then(|x| x.as_u64()))
                    .collect::<Vec<_>>()
            );
        }
        Ok(json!({ "records": items }))
    }

    /// 注册插件通知地址(推送式投递;附着模式或需要晚注册时使用)
    async fn set_notify_url(&self, body: &Value) -> Result<Value, String> {
        let url = body
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let state = self.handle.state::<crate::ui::UiState>();
        *state.notify_url.lock().unwrap() = if url.is_empty() {
            None
        } else {
            Some(url.clone())
        };
        if url.is_empty() {
            eprintln!("[notify] notify-url cleared");
        } else {
            eprintln!("[notify] notify-url set: {url}");
        }
        Ok(json!({ "ok": true, "url": url }))
    }
}

/// 提取字符串字段
#[allow(dead_code)]
fn str(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{phase_arg, tab_id_for_index, validate_reveal};
    use serde_json::json;

    /// `--tab` 为 0 基索引:按当前标签顺序映射到内部 id,越界明确报错
    #[test]
    fn tab_index_maps_to_internal_id_and_rejects_overflow() {
        let ids = [1u32, 5, 9];
        assert_eq!(tab_id_for_index(&ids, 0).unwrap(), 1);
        assert_eq!(tab_id_for_index(&ids, 1).unwrap(), 5);
        assert_eq!(tab_id_for_index(&ids, 2).unwrap(), 9);
        let err = tab_id_for_index(&ids, 3).unwrap_err();
        assert!(err.contains("tab index out of range"), "got: {err}");
        assert!(tab_id_for_index(&[], 0).is_err());
    }

    /// phase 参数:合法值透传,非法值报错,非字符串报错
    #[test]
    fn phase_argument_is_validated() {
        assert_eq!(phase_arg(&json!({})).unwrap(), "");
        assert_eq!(phase_arg(&json!({"phase": "load"})).unwrap(), "load");
        assert_eq!(
            phase_arg(&json!({"phase": "action:click"})).unwrap(),
            "action:click"
        );
        assert!(phase_arg(&json!({"phase": "bogus"})).is_err());
        assert!(phase_arg(&json!({"phase": "action:"})).is_err());
        assert!(phase_arg(&json!({"phase": 1})).is_err());
    }

    /// revealSecrets 必须配合 includeHeaders
    #[test]
    fn reveal_secrets_requires_include_headers() {
        assert!(validate_reveal(true, true).is_ok());
        assert!(validate_reveal(false, false).is_ok());
        let err = validate_reveal(false, true).unwrap_err();
        assert!(err.contains("--include-headers"), "got: {err}");
    }
}
