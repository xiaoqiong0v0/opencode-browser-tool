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

use serde_json::Value;
use tauri::AppHandle;

use crate::ui;

/// 页面操作错误
pub type ControlResult<T> = Result<T, String>;

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

/// 滚动页面(dx/dy 为视口 CSS 像素偏移)
/// JS 必须返回可 JSON 化值:eval 会对返回值做 JSON 解析,window.scrollBy 返回 undefined 会报 EOF
pub fn scroll(app: &AppHandle, dx: i32, dy: i32) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{ window.scrollBy({{left:{dx},top:{dy}}}); return {{ok:true}}; }})()"#
    );
    eval(app, &js)?;
    Ok(())
}

/// 滚动到元素
pub fn scroll_to_element(app: &AppHandle, selector: &str) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          const el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          el.scrollIntoView({{behavior:"smooth", block:"center"}});
          return {{ok:true}};
        }})()"#,
        sel = selector
    );
    let v = eval(app, &js)?;
    if v.get("error").is_some() {
        return Err(v["error"].as_str().unwrap_or("scroll failed").to_string());
    }
    Ok(())
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
