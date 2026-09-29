//! 控制协议层:通过 eval 驱动页面 Webview 完成页面操作(跨平台统一)
//! 所有操作在页面内执行一次性 JS,无残留脚本;截图平台分支
//! 注意:eval_with_callback 会把 JS 表达式返回值 JSON 序列化后传回,
//! 因此 JS 表达式直接返回对象/原始值,不要再包 JSON.stringify
pub mod accessibility;
/// 媒体权限放行(Windows/Linux:摄像头/麦克风统一放行,配合页面模拟脚本)
#[cfg(any(windows, target_os = "linux"))]
pub mod media;
pub mod responses;
pub mod screenshot;

use serde_json::Value;
use tauri::AppHandle;

use crate::ui;

/// 页面操作错误
pub type ControlResult<T> = Result<T, String>;

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

/// 点击元素(selector 定位)
pub fn click(app: &AppHandle, selector: &str) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          const el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          el.scrollIntoView({{block:"center"}});
          el.click();
          return {{ok:true}};
        }})()"#,
        sel = selector
    );
    let v = eval(app, &js)?;
    if v.get("error").is_some() {
        return Err(v["error"].as_str().unwrap_or("click failed").to_string());
    }
    Ok(())
}

/// 填写输入框
pub fn fill(app: &AppHandle, selector: &str, value: &str) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          const el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
          setter.call(el, {val:?});
          el.dispatchEvent(new Event("input", {{bubbles:true}}));
          el.dispatchEvent(new Event("change", {{bubbles:true}}));
          return {{ok:true}};
        }})()"#,
        sel = selector,
        val = value
    );
    let v = eval(app, &js)?;
    if v.get("error").is_some() {
        return Err(v["error"].as_str().unwrap_or("fill failed").to_string());
    }
    Ok(())
}

/// 获取可见文本
pub fn visible_text(app: &AppHandle) -> ControlResult<String> {
    let v = eval(app, "document.body ? document.body.innerText : ''")?;
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

/// 滚动页面
pub fn scroll(app: &AppHandle, dx: i32, dy: i32) -> ControlResult<()> {
    eval(
        app,
        &format!("window.scrollBy({{left:{dx},top:{dy}}})"),
    )?;
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
