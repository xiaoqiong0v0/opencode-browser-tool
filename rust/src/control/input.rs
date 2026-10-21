//! 文本输入与下拉选择:可信优先,无可信通道时降级到 untrusted DOM/JS 并标注
//!
//! Windows(可信,复用 `control::cdp` / `control::keyboard`):
//!   - `fill`   :focus → 全选 → CDP `Input.insertText` → 读回校验
//!   - `clear`  :focus → 全选 → CDP `Input.dispatchKeyEvent`(Delete) → 读回校验
//!   - `select` :focus → 方向键移动到目标 option → 读回校验;`multiple`/`size>1` 无可信键盘路径 → 降级
//!   - iframe_fill:在 iframe 内容文档内 focus → insertText → 读回
//! 非 Windows(降级):原生原型 setter + 派发 `input`/`change`(与旧实现一致),返回标注 `degraded`

use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::AppHandle;

use super::{ControlResult, Outcome, trusted_available};
use super::keyboard;

/// 填写输入框:可信通道失败时报错,降级路径标注 degraded
pub fn fill(app: &AppHandle, selector: &str, value: &str) -> ControlResult<Outcome<()>> {
    fill_with(app, selector, value, None)
}

/// 同 `fill`,`timeout_ms` 为 Some 时贯穿整条链:聚焦/全选 eval、insertText CDP、读回校验轮询
/// 读回校验预算取 `min(传入, VERIFY_TIMEOUT_MS)` —— 不传则与 1.8.x 完全一致
pub fn fill_with(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Outcome<()>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    if trusted_available() {
        fill_trusted(app, selector, value, timeout_ms)?;
        return Ok(Outcome::trusted(()));
    }
    fill_fallback(app, selector, value, timeout_ms)?;
    Ok(Outcome::degraded(()))
}

/// 清空输入框(等价 fill 空值;Windows 用全选 + Delete,确保产生 input 事件)
pub fn clear(app: &AppHandle, selector: &str) -> ControlResult<Outcome<()>> {
    clear_with(app, selector, None)
}

/// 同 `clear`,`timeout_ms` 口径与 `fill_with` 一致
pub fn clear_with(
    app: &AppHandle,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Outcome<()>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    if trusted_available() {
        clear_trusted(app, selector, timeout_ms)?;
        return Ok(Outcome::trusted(()));
    }
    fill_fallback(app, selector, "", timeout_ms)?;
    Ok(Outcome::degraded(()))
}

/// 选择下拉项:返回读回的实际选中值(便于调用方校验)
pub fn select(app: &AppHandle, selector: &str, value: &str) -> ControlResult<Outcome<String>> {
    select_with(app, selector, value, None)
}

/// 同 `select`,`timeout_ms` 口径与 `fill_with` 一致(方向键次数 × 单次 CDP 预算 + 读回轮询)
pub fn select_with(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Outcome<String>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    if trusted_available() {
        if let Some(actual) = select_trusted_if_supported(app, selector, value, timeout_ms)? {
            return Ok(Outcome::trusted(actual));
        }
        // multiple / size>1:可信键盘路径不适用,降级为 DOM 赋值并明确标注
        let actual = select_fallback(app, selector, value, timeout_ms)?;
        return Ok(Outcome::degraded_with(
            actual,
            "untrusted JS fallback (multi-select / size>1 has no trusted keyboard path)",
        ));
    }
    let actual = select_fallback(app, selector, value, timeout_ms)?;
    Ok(Outcome::degraded(actual))
}

/// 在 iframe 中填写输入框
pub fn fill_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
) -> ControlResult<Outcome<()>> {
    fill_in_iframe_with(app, iframe_selector, selector, value, None)
}

/// 同 `fill_in_iframe`,`timeout_ms` 口径与 `fill_with` 一致
pub fn fill_in_iframe_with(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Outcome<()>> {
    if iframe_selector.is_empty() {
        return Err("iframeSelector is required".into());
    }
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    if trusted_available() {
        fill_in_iframe_trusted(app, iframe_selector, selector, value, timeout_ms)?;
        return Ok(Outcome::trusted(()));
    }
    fill_in_iframe_fallback(app, iframe_selector, selector, value, timeout_ms)?;
    Ok(Outcome::degraded(()))
}

// ---- 降级实现(所有平台编译,Windows 强制降级时可实测) ----

/// 原生原型 setter 赋值 + 派发 input/change(untrusted)
fn fill_fallback(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
          var setter = Object.getOwnPropertyDescriptor(proto, "value").set;
          if (!setter) return {{error: "no value setter"}};
          setter.call(el, {val:?});
          el.dispatchEvent(new Event("input", {{bubbles:true}}));
          el.dispatchEvent(new Event("change", {{bubbles:true}}));
          return {{ok:true}};
        }})()"#,
        sel = selector,
        val = value
    );
    super::eval_with(app, &js, timeout_ms)?;
    Ok(())
}

/// DOM 赋值 select + 派发 input/change(untrusted),返回读回值
fn select_fallback(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<String> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set;
          if (setter) setter.call(el, {val:?}); else el.value = {val:?};
          el.dispatchEvent(new Event("input", {{bubbles:true}}));
          el.dispatchEvent(new Event("change", {{bubbles:true}}));
          return {{ok:true, value: el.value}};
        }})()"#,
        sel = selector,
        val = value
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    check_error(&v)?;
    Ok(v.get("value").and_then(Value::as_str).unwrap_or("").to_string())
}

/// iframe 内 DOM 赋值 + 派发 input/change(untrusted)
fn fill_in_iframe_fallback(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var f = document.querySelector({iframe:?});
          if (!f) return {{error: "iframe not found"}};
          var d = f.contentDocument;
          if (!d) return {{error: "cross-origin iframe not accessible"}};
          var el = d.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var proto = el.tagName === "TEXTAREA" ? d.defaultView.HTMLTextAreaElement.prototype : d.defaultView.HTMLInputElement.prototype;
          var setter = Object.getOwnPropertyDescriptor(proto, "value").set;
          setter.call(el, {val:?});
          el.dispatchEvent(new d.defaultView.Event("input", {{bubbles:true}}));
          el.dispatchEvent(new d.defaultView.Event("change", {{bubbles:true}}));
          return {{ok:true}};
        }})()"#,
        iframe = iframe_selector,
        sel = selector,
        val = value
    );
    super::eval_with(app, &js, timeout_ms)?;
    Ok(())
}

// ---- Windows 可信实现 ----

/// 可信 fill:focus → 全选 → insertText → 读回校验
fn fill_trusted(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    keyboard::focus_element_with(app, selector, timeout_ms)?;
    select_all(app, selector, timeout_ms)?;
    if value.is_empty() {
        // 空值:用 Delete 清掉选中内容(insertText("") 可能为空操作)
        keyboard::press_key_with(app, "Delete", "", timeout_ms)?;
    } else {
        keyboard::insert_text_with(app, value, timeout_ms)?;
    }
    verify_value(app, selector, value, timeout_ms)
}

/// 可信 clear:focus → 全选 → Delete → 读回校验
fn clear_trusted(app: &AppHandle, selector: &str, timeout_ms: Option<u64>) -> ControlResult<()> {
    keyboard::focus_element_with(app, selector, timeout_ms)?;
    select_all(app, selector, timeout_ms)?;
    keyboard::press_key_with(app, "Delete", "", timeout_ms)?;
    verify_value(app, selector, "", timeout_ms)
}

/// 可信 select:仅单值 select 适用;返回 Some(实际值);`multiple`/`size>1` 返回 None 交给降级
fn select_trusted_if_supported(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Option<String>> {
    let info = select_info(app, selector, value, timeout_ms)?;
    if info.multiple || info.size > 1 {
        return Ok(None);
    }
    if info.target_index < 0 {
        return Err(format!("option not found: {value}"));
    }
    keyboard::focus_element_with(app, selector, timeout_ms)?;
    let delta = info.target_index - info.selected_index;
    let key = if delta >= 0 { "ArrowDown" } else { "ArrowUp" };
    for _ in 0..delta.abs() {
        keyboard::press_key_with(app, key, "", timeout_ms)?;
    }
    // 读回校验预算:传入值与 VERIFY_TIMEOUT_MS 取小 → 不传维持既有 2500ms,传小值可快失败
    let actual = wait_value(app, selector, value, verify_budget(timeout_ms))?;
    if actual != value {
        return Err(format!(
            "select verification failed: expected {value:?}, got {actual:?} \
             (trusted keyboard path did not change selection)"
        ));
    }
    Ok(Some(actual))
}

/// 可信 iframe_fill:iframe 内 focus → 全选 → insertText → 读回
fn fill_in_iframe_trusted(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    focus_in_iframe(app, iframe_selector, selector, timeout_ms)?;
    select_all_in_iframe(app, iframe_selector, selector, timeout_ms)?;
    if value.is_empty() {
        keyboard::press_key_with(app, "Delete", "", timeout_ms)?;
    } else {
        keyboard::insert_text_with(app, value, timeout_ms)?;
    }
    let actual =
        wait_value_in_iframe(app, iframe_selector, selector, value, verify_budget(timeout_ms))?;
    if actual != value {
        return Err(format!(
            "iframe fill verification failed: expected {value:?}, got {actual:?}"
        ));
    }
    Ok(())
}

/// 选中目标元素全部内容(input/textarea 用 select()/setSelectionRange,contenteditable 用 Range)
fn select_all(app: &AppHandle, selector: &str, timeout_ms: Option<u64>) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          try {{ if (el.select) el.select(); }} catch (e) {{}}
          try {{
            if (typeof el.setSelectionRange === "function" && typeof el.value === "string") {{
              el.setSelectionRange(0, el.value.length);
            }}
          }} catch (e) {{}}
          if (el.isContentEditable) {{
            var r = document.createRange(); r.selectNodeContents(el);
            var s = window.getSelection(); s.removeAllRanges(); s.addRange(r);
          }}
          return {{ok:true}};
        }})()"#,
        sel = selector
    );
    super::eval_with(app, &js, timeout_ms)?;
    Ok(())
}

/// 读取元素当前值(input/textarea 取 value,contenteditable 取 textContent)
fn read_value(app: &AppHandle, selector: &str, timeout_ms: Option<u64>) -> ControlResult<String> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          return {{value: (typeof el.value === "string") ? el.value : (el.textContent || "")}};
        }})()"#,
        sel = selector
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    check_error(&v)?;
    Ok(v.get("value").and_then(Value::as_str).unwrap_or("").to_string())
}

/// 输入事件异步处理(WebKitGTK 跨进程)时,读回校验的最长等待时间
const VERIFY_TIMEOUT_MS: u64 = 2500;

/// 读回校验预算:模型传入值与 `VERIFY_TIMEOUT_MS` 取小 —— 不传维持既有 2500ms,传小值即可快失败
fn verify_budget(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .map(|v| v.min(VERIFY_TIMEOUT_MS))
        .unwrap_or(VERIFY_TIMEOUT_MS)
}

/// 轮询读取元素值直到等于期望或超时(超时返回当前值,由调用方判错)
///
/// Windows CDP 注入是同步的,首次读取即命中;Linux GDK 注入后 WebKit 跨进程异步更新 DOM,
/// 需要轮询等待(否则会把"尚未处理"误判为失败)。轮询 sleep 在 worker 线程,主循环照常处理事件。
fn wait_value(
    app: &AppHandle,
    selector: &str,
    expected: &str,
    timeout_ms: u64,
) -> ControlResult<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        // 轮询内每步读回 eval 的预算 = 本轮等待预算(两者同量级,避免单步 eval 拖垮整轮)
        let actual = read_value(app, selector, Some(timeout_ms))?;
        if actual == expected || Instant::now() >= deadline {
            return Ok(actual);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// iframe 版本的轮询读回
fn wait_value_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    expected: &str,
    timeout_ms: u64,
) -> ControlResult<String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let actual = read_value_in_iframe(app, iframe_selector, selector, Some(timeout_ms))?;
        if actual == expected || Instant::now() >= deadline {
            return Ok(actual);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

/// 读回校验:实际值必须等于期望值,否则报错(不静默成功)
fn verify_value(
    app: &AppHandle,
    selector: &str,
    expected: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let actual = wait_value(app, selector, expected, verify_budget(timeout_ms))?;
    if actual != expected {
        return Err(format!(
            "fill verification failed: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

/// select 元信息(供可信键盘路径判断是否适用)
struct SelectInfo {
    multiple: bool,
    size: i64,
    selected_index: i64,
    target_index: i64,
}

/// 查询 select 的 multiple/size/当前索引/目标值所在索引;未找到目标值 target_index = -1
fn select_info(
    app: &AppHandle,
    selector: &str,
    value: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<SelectInfo> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          if (el.tagName !== "SELECT") return {{error: "not a select element"}};
          var idx = -1;
          for (var i = 0; i < el.options.length; i++) {{
            if (el.options[i].value === {val:?}) {{ idx = i; break; }}
          }}
          return {{multiple: !!el.multiple, size: el.size || 0, selectedIndex: el.selectedIndex, targetIndex: idx}};
        }})()"#,
        sel = selector,
        val = value
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    check_error(&v)?;
    Ok(SelectInfo {
        multiple: v.get("multiple").and_then(Value::as_bool).unwrap_or(false),
        size: v.get("size").and_then(Value::as_i64).unwrap_or(0),
        selected_index: v.get("selectedIndex").and_then(Value::as_i64).unwrap_or(0),
        target_index: v.get("targetIndex").and_then(Value::as_i64).unwrap_or(-1),
    })
}

/// iframe 内容文档内聚焦元素
fn focus_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var f = document.querySelector({iframe:?});
          if (!f) return {{error: "iframe not found"}};
          var d = f.contentDocument;
          if (!d) return {{error: "cross-origin iframe not accessible"}};
          var el = d.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          if (f.scrollIntoView) f.scrollIntoView({{block: "center"}});
          if (el.scrollIntoView) el.scrollIntoView({{block: "center"}});
          if (el.focus) el.focus();
          return {{ok: d.activeElement === el}};
        }})()"#,
        iframe = iframe_selector,
        sel = selector
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    check_error(&v)?;
    if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("element is not focusable in iframe: {selector}"));
    }
    Ok(())
}

/// iframe 内容文档内全选
fn select_all_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var f = document.querySelector({iframe:?});
          if (!f) return {{error: "iframe not found"}};
          var d = f.contentDocument;
          if (!d) return {{error: "cross-origin iframe not accessible"}};
          var el = d.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          try {{ if (el.select) el.select(); }} catch (e) {{}}
          try {{
            if (typeof el.setSelectionRange === "function" && typeof el.value === "string") {{
              el.setSelectionRange(0, el.value.length);
            }}
          }} catch (e) {{}}
          return {{ok:true}};
        }})()"#,
        iframe = iframe_selector,
        sel = selector
    );
    super::eval_with(app, &js, timeout_ms)?;
    Ok(())
}

/// iframe 内容文档内读取 value/textContent
fn read_value_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<String> {
    let js = format!(
        r#"(function(){{
          var f = document.querySelector({iframe:?});
          if (!f) return {{error: "iframe not found"}};
          var d = f.contentDocument;
          if (!d) return {{error: "cross-origin iframe not accessible"}};
          var el = d.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          return {{value: (typeof el.value === "string") ? el.value : (el.textContent || "")}};
        }})()"#,
        iframe = iframe_selector,
        sel = selector
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    check_error(&v)?;
    Ok(v.get("value").and_then(Value::as_str).unwrap_or("").to_string())
}

/// JS 返回值里带 error 字段则转 Err
fn check_error(v: &Value) -> ControlResult<()> {
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    Ok(())
}
