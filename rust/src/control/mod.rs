//! 控制协议层:通过 eval 驱动页面 Webview 完成页面操作(跨平台统一)
//! 所有操作在页面内执行一次性 JS,无残留脚本;截图平台分支
//! 注意:eval_with_callback 会把 JS 表达式返回值 JSON 序列化后传回,
//! 因此 JS 表达式直接返回对象/原始值,不要再包 JSON.stringify
pub mod accessibility;
/// Windows CDP 通用调用通道(截图/可访问性树/键盘注入共用)
#[cfg(windows)]
pub mod cdp;
/// 拖拽(仅 Windows:CDP 鼠标序列触发原生 HTML5 DnD)
pub mod drag;
/// Linux GDK 可信输入通道(gdk_event_put 注入键盘/指针事件)
#[cfg(target_os = "linux")]
pub mod gdk;
/// 键盘注入(Windows:CDP;Linux:GDK;其余平台明确报错)
pub mod keyboard;
/// 文本输入与下拉选择(Windows:CDP 可信通道;非 Windows 降级到 DOM/JS 并标注)
pub mod input;
/// 媒体权限放行(Windows/Linux:摄像头/麦克风统一放行,配合页面模拟脚本)
#[cfg(any(windows, target_os = "linux"))]
pub mod media;
/// 可信鼠标注入(Windows:CDP Input.dispatchMouseEvent;点击/悬停/iframe 点击)
pub mod mouse;
pub mod responses;
pub mod screenshot;
/// 文件上传(Windows:CDP DOM.setFileInputFiles)
pub mod upload;

use serde_json::{json, Value};
use tauri::AppHandle;

use crate::ui;

/// 页面操作错误
pub type ControlResult<T> = Result<T, String>;

/// 滚动/就绪轮询间隔(ms)
const POLL_MS: u64 = 50;
/// 判定"滚动已稳定"所需的连续相同采样次数
const SCROLL_STABLE_SAMPLES: u32 = 3;
/// 滚动稳定等待默认超时(ms),可用 `BT_SCROLL_TIMEOUT_MS` 覆盖(调试)
const DEFAULT_SCROLL_TIMEOUT_MS: u64 = 3000;
/// 导航就绪等待默认超时(ms),可用 `BT_NAV_TIMEOUT_MS` 覆盖(调试)
const DEFAULT_NAV_TIMEOUT_MS: u64 = 15000;
/// 后退/前进就绪等待超时(ms)
const HISTORY_TIMEOUT_MS: u64 = 5000;

/// 调试用超时覆盖:读环境变量(正整数),非法/缺省时用 default
pub fn timeout_from_env(var: &str, default_ms: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default_ms)
}

/// 滚动稳定等待超时(ms)
pub fn scroll_timeout_ms() -> u64 {
    timeout_from_env("BT_SCROLL_TIMEOUT_MS", DEFAULT_SCROLL_TIMEOUT_MS)
}

/// 导航就绪等待超时(ms)
pub fn nav_timeout_ms() -> u64 {
    timeout_from_env("BT_NAV_TIMEOUT_MS", DEFAULT_NAV_TIMEOUT_MS)
}

/// 前进/后退就绪等待超时(ms)
pub fn history_timeout_ms() -> u64 {
    timeout_from_env("BT_HISTORY_TIMEOUT_MS", HISTORY_TIMEOUT_MS)
}

/// 控制操作结果:附带"降级"说明
/// - `degraded = None`:走可信/原生通道(Windows CDP)
/// - `degraded = Some(reason)`:无可信通道,退回 untrusted DOM/JS 实现(须在返回里标注)
#[derive(Debug, Clone)]
pub struct Outcome<T> {
    /// 操作本体(坐标 / 读回值等)
    pub value: T,
    /// 降级原因;None 表示可信/原生
    pub degraded: Option<String>,
}

impl<T> Outcome<T> {
    /// 可信/原生执行
    pub fn trusted(value: T) -> Self {
        Self { value, degraded: None }
    }

    /// 降级执行(untrusted DOM/JS 回退)
    pub fn degraded(value: T) -> Self {
        Self { value, degraded: Some(degrade_reason()) }
    }

    /// 降级执行并附自定义原因(如 Windows 上个别场景无适用可信路径)
    pub fn degraded_with(value: T, reason: impl Into<String>) -> Self {
        Self { value, degraded: Some(reason.into()) }
    }
}

/// 统一降级原因文本(键 = degradedReason,插件据此追加 `(degraded: ...)`)
pub fn degrade_reason() -> String {
    format!("untrusted JS fallback on {}", std::env::consts::OS)
}

/// 调试开关:置 `BT_FORCE_DEGRADED=1` 时即使在 Windows 也强制走降级分支
/// (用于在无 Linux 的环境下实测降级分支的代码路径与标注文本)
pub fn force_degraded() -> bool {
    std::env::var("BT_FORCE_DEGRADED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 是否可走可信通道
/// - Windows:CDP
/// - Linux:GDK 事件注入(`gdk_event_put`,已实测可信)
/// - macOS:无 → false
/// 置 `BT_FORCE_DEGRADED=1` 时强制 false(便于实测降级分支)
pub fn trusted_available() -> bool {
    (cfg!(windows) || cfg!(target_os = "linux")) && !force_degraded()
}

/// 执行页面内 JS,返回解析后的值(对象/null/字符串等)
pub fn eval(app: &AppHandle, js: &str) -> ControlResult<Value> {
    let raw = ui::eval_page(app, js)?;
    serde_json::from_str(&raw).map_err(|e| format!("parse eval result: {e}"))
}

/// 导航到 URL
pub fn navigate(app: &AppHandle, url: &str) -> ControlResult<()> {
    ui::active_page_webview(app)
        .ok_or("page webview not ready")?
        .navigate(url.parse().map_err(|e| format!("invalid url: {e}"))?)
        .map_err(|e| format!("navigate failed: {e}"))
}

/// 执行任意 JS,返回结果(自动 JSON 序列化)
pub fn evaluate(app: &AppHandle, script: &str) -> ControlResult<Value> {
    eval(app, &format!("(function(){{ return {script}; }})()"))
}

/// 获取可见文本
/// selector 为 None/空:取 document.body 全文;指定时取该元素 innerText(找不到返回明确错误)
pub fn visible_text(app: &AppHandle, selector: Option<&str>) -> ControlResult<String> {
    let js = match selector {
        Some(s) if !s.is_empty() => format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el) return {{error: "element not found"}};
              return el.innerText;
            }})()"#,
            s = s
        ),
        _ => "document.body ? document.body.innerText : ''".to_string(),
    };
    let v = eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    Ok(v.as_str().unwrap_or("").to_string())
}

/// 元素状态(可见性/是否禁用/尺寸)
pub fn element_state(app: &AppHandle, selector: &str) -> ControlResult<Value> {
    eval(
        app,
        &format!(
            r#"(function(){{
              const el = document.querySelector({sel:?});
              if (!el) return {{exists:false}};
              const r = el.getBoundingClientRect();
              return {{
                exists:true, visible: r.width>0 && r.height>0,
                disabled: !!el.disabled, tag: el.tagName.toLowerCase(),
                rect: {{x:r.x,y:r.y,w:r.width,h:r.height}}
              }};
            }})()"#,
            sel = selector
        ),
    )
}

/// 文档就绪状态快照 {readyState,url,title}
pub fn document_state(app: &AppHandle) -> ControlResult<Value> {
    eval(
        app,
        "(function(){return {readyState: document.readyState, url: location.href, title: document.title};})()",
    )
}

/// 等待页面就绪:readyState 达到 interactive/complete 且已观察到导航进展
///
/// 进展 = URL 相对 `prev_url` 已变化(SPA/导航),或曾观察到 `readyState=="loading"`。
/// `prev_url` 传空串表示不要求 URL 变化(仅要求 isLoading→loaded,并留 150ms 最小观察窗)。
/// 返回 `{ready,readyState,url,title,timedOut,waitedMs}`;超时如实返回 `ready:false`(不假装成功)。
pub fn wait_ready(app: &AppHandle, prev_url: &str, timeout_ms: u64) -> ControlResult<Value> {
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_millis(timeout_ms);
    let mut saw_loading = false;
    let mut saw_change = false;
    loop {
        let st = document_state(app)?;
        let rs = st.get("readyState").and_then(Value::as_str).unwrap_or("loading").to_string();
        let url = st.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let title = st.get("title").and_then(Value::as_str).unwrap_or("").to_string();
        if rs == "loading" {
            saw_loading = true;
        }
        if !prev_url.is_empty() && url != prev_url {
            saw_change = true;
        }
        let loaded = rs == "interactive" || rs == "complete";
        // 最小观察窗:prev_url 为空(如同一 URL 导航)时避免命中"旧文档已完成"
        let min_window_ok = !prev_url.is_empty() || start.elapsed().as_millis() >= 150;
        if loaded && (saw_change || saw_loading || min_window_ok) {
            return Ok(json!({
                "ready": true, "readyState": rs, "url": url, "title": title,
                "timedOut": false, "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({
                "ready": false, "readyState": rs, "url": url, "title": title,
                "timedOut": true, "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
    }
}

/// 读取窗口滚动位置四舍五入值 (x, y)
fn scroll_pos(app: &AppHandle) -> ControlResult<(i64, i64)> {
    let v = eval(
        app,
        "(function(){return {x: Math.round(window.scrollX), y: Math.round(window.scrollY)};})()",
    )?;
    Ok((
        v.get("x").and_then(Value::as_i64).unwrap_or(0),
        v.get("y").and_then(Value::as_i64).unwrap_or(0),
    ))
}

/// 滚动页面(dx/dy 为视口 CSS 像素偏移)并等待位置稳定
///
/// 滚动是异步的(尤其 `scroll-behavior: smooth`),发出后轮询窗口滚动位置,
/// 连续 `SCROLL_STABLE_SAMPLES` 次采样不变视为稳定;超时则如实返回 `stable:false`。
/// 返回 `{stable,timedOut,x,y,waitedMs}`
pub fn scroll(app: &AppHandle, dx: i32, dy: i32) -> ControlResult<Value> {
    let js = format!(
        r#"(function(){{ window.scrollBy({{left:{dx},top:{dy}}}); return {{ok:true}}; }})()"#
    );
    eval(app, &js)?;
    let timeout = scroll_timeout_ms();
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_millis(timeout);
    let mut last = scroll_pos(app)?;
    let mut stable = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        let cur = scroll_pos(app)?;
        if cur == last {
            stable += 1;
        } else {
            stable = 0;
            last = cur;
        }
        if stable >= SCROLL_STABLE_SAMPLES {
            return Ok(json!({
                "stable": true, "timedOut": false,
                "x": cur.0, "y": cur.1,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({
                "stable": false, "timedOut": true,
                "x": cur.0, "y": cur.1,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
    }
}

/// 元素是否在视口内(有尺寸且与视口矩形相交)
fn element_in_viewport(app: &AppHandle, selector: &str) -> ControlResult<bool> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var r = el.getBoundingClientRect();
          var vw = window.innerWidth, vh = window.innerHeight;
          var ok = r.width > 0 && r.height > 0 && r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw;
          return {{inViewport: !!ok}};
        }})()"#,
        sel = selector
    );
    let v = eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    Ok(v.get("inViewport").and_then(Value::as_bool).unwrap_or(false))
}

/// 兜底显式滚动后的有界等待(ms)
const FALLBACK_SCROLL_TIMEOUT_MS: u64 = 800;

/// 轮询等待元素进入视口且窗口滚动稳定;返回 `(reached, stable)`
fn wait_element_in_view(
    app: &AppHandle,
    selector: &str,
    timeout_ms: u64,
) -> ControlResult<(bool, bool)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut last = scroll_pos(app)?;
    let mut stable = 0u32;
    loop {
        let in_view = element_in_viewport(app, selector)?;
        if in_view && stable >= SCROLL_STABLE_SAMPLES {
            return Ok((true, true));
        }
        if std::time::Instant::now() >= deadline {
            return Ok((false, stable >= SCROLL_STABLE_SAMPLES));
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        let cur = scroll_pos(app)?;
        if cur == last {
            stable += 1;
        } else {
            stable = 0;
            last = cur;
        }
    }
}

/// 兜底 JS:显式把元素在各级可滚动祖先(内层容器)居中,再对窗口 `scrollBy`
fn bring_ancestors_into_view_js(selector: &str) -> String {
    format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var node = el.parentElement;
          while (node && node !== document.body && node !== document.documentElement) {{
            var cs = getComputedStyle(node);
            var oy = cs.overflowY;
            if ((oy === "auto" || oy === "scroll" || oy === "overlay") &&
                node.scrollHeight > node.clientHeight + 1) {{
              var r = el.getBoundingClientRect(), nr = node.getBoundingClientRect();
              node.scrollTop += (r.top - nr.top) - (node.clientHeight - r.height) / 2;
            }}
            node = node.parentElement;
          }}
          var r = el.getBoundingClientRect();
          window.scrollBy(0, r.top - (window.innerHeight - r.height) / 2);
          return {{ok:true}};
        }})()"#,
        sel = selector
    )
}

/// 滚动到元素并等待其真正进入视口且滚动位置稳定
///
/// 主路径用**立即**(非 smooth)的 `scrollIntoView({block:center,inline:center})`,一次性处理元素的
/// 所有滚动祖先与窗口(实测平滑滚动在**嵌套滚动容器**上不可靠:窗口可能停在中途,元素仍在视口外);
/// 若仍未进入视口,再显式逐级滚动可滚动祖先 + 窗口。全程有界超时,超时如实返回 `reached:false`
/// 并给 `reason`。
pub fn scroll_to_element(app: &AppHandle, selector: &str) -> ControlResult<Value> {
    let start = std::time::Instant::now();
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          el.scrollIntoView({{block:"center", inline:"center"}});
          return {{ok:true}};
        }})()"#,
        sel = selector
    );
    let v = eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let (reached, stable) = wait_element_in_view(app, selector, scroll_timeout_ms())?;
    if reached {
        return Ok(json!({
            "reached": true, "inViewport": true, "stable": stable, "timedOut": false,
            "waitedMs": start.elapsed().as_millis() as u64,
        }));
    }
    // 兜底:显式滚动可滚动祖先链 + 窗口,再做一次有界等待
    let _ = eval(app, &bring_ancestors_into_view_js(selector));
    let (reached2, stable2) = wait_element_in_view(app, selector, FALLBACK_SCROLL_TIMEOUT_MS)?;
    let in_view = element_in_viewport(app, selector)?;
    let mut out = json!({
        "reached": reached2, "inViewport": in_view, "stable": stable2, "timedOut": !reached2,
        "waitedMs": start.elapsed().as_millis() as u64,
    });
    if !reached2 {
        out["reason"] = json!(
            "element not brought into view (possibly outside the document's scrollable range)"
        );
    }
    Ok(out)
}

/// 等待元素出现(轮询,最多 timeout_ms)
pub fn wait_for_selector(
    app: &AppHandle,
    selector: &str,
    timeout_ms: u64,
) -> ControlResult<bool> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let v = eval(
            app,
            &format!("!!document.querySelector({sel:?})", sel = selector),
        )?;
        if v.as_bool().unwrap_or(false) {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 页面状态快照(标题/URL/视口)
pub fn page_state(app: &AppHandle) -> ControlResult<Value> {
    eval(app, "window.__btPage ? __btPage.state() : null")
}

/// 查询元素信息(elementFromPoint + selector 生成)
/// 返回解析后的 JSON 对象或 null
pub fn query_element(app: &AppHandle, x: i32, y: i32) -> ControlResult<Value> {
    eval(
        app,
        &format!("window.__btPage ? __btPage.query({x},{y}) : null"),
    )
}
