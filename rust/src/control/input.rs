//! 文本输入与下拉选择:可信优先,无可信通道时降级到 untrusted DOM/JS 并标注
//!
//! Windows(可信,复用 `control::cdp` / `control::keyboard`):
//!   - `fill`   :focus → 全选 → CDP `Input.insertText` → 读回校验
//!   - `clear`  :focus → 全选 → CDP `Input.dispatchKeyEvent`(Delete) → 读回校验
//!   - `select` :focus → 方向键移动到目标 option → 读回校验;`multiple`/`size>1` 无可信键盘路径 → 降级
//!   - iframe_fill:在 iframe 内容文档内 focus → insertText → 读回
//! 非 Windows(降级):原生原型 setter + 派发 `input`/`change`(与旧实现一致),返回标注 `degraded`

use serde_json::Value;
use tauri::AppHandle;

use super::{ControlResult, Outcome};

#[cfg(windows)]
use super::trusted_available;
#[cfg(windows)]
use super::{cdp, keyboard};

/// 填写输入框:可信通道失败时报错,降级路径标注 degraded
pub fn fill(app: &AppHandle, selector: &str, value: &str) -> ControlResult<Outcome<()>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    #[cfg(windows)]
    if trusted_available() {
        fill_trusted(app, selector, value)?;
        return Ok(Outcome::trusted(()));
    }
    fill_fallback(app, selector, value)?;
    Ok(Outcome::degraded(()))
}

/// 清空输入框(等价 fill 空值;Windows 用全选 + Delete,确保产生 input 事件)
pub fn clear(app: &AppHandle, selector: &str) -> ControlResult<Outcome<()>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    #[cfg(windows)]
    if trusted_available() {
        clear_trusted(app, selector)?;
        return Ok(Outcome::trusted(()));
    }
    fill_fallback(app, selector, "")?;
    Ok(Outcome::degraded(()))
}

/// 选择下拉项:返回读回的实际选中值(便于调用方校验)
pub fn select(app: &AppHandle, selector: &str, value: &str) -> ControlResult<Outcome<String>> {
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    #[cfg(windows)]
    if trusted_available() {
        if let Some(actual) = select_trusted_if_supported(app, selector, value)? {
            return Ok(Outcome::trusted(actual));
        }
        // multiple / size>1:可信键盘路径不适用,降级为 DOM 赋值并明确标注
        let actual = select_fallback(app, selector, value)?;
        return Ok(Outcome::degraded_with(
            actual,
            "untrusted JS fallback (multi-select / size>1 has no trusted keyboard path)",
        ));
    }
    let actual = select_fallback(app, selector, value)?;
    Ok(Outcome::degraded(actual))
}

/// 在 iframe 中填写输入框
pub fn fill_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
) -> ControlResult<Outcome<()>> {
    if iframe_selector.is_empty() {
        return Err("iframeSelector is required".into());
    }
    if selector.is_empty() {
        return Err("selector is required".into());
    }
    #[cfg(windows)]
    if trusted_available() {
        fill_in_iframe_trusted(app, iframe_selector, selector, value)?;
        return Ok(Outcome::trusted(()));
    }
    fill_in_iframe_fallback(app, iframe_selector, selector, value)?;
    Ok(Outcome::degraded(()))
}

// ---- 降级实现(所有平台编译,Windows 强制降级时可实测) ----

/// 原生原型 setter 赋值 + 派发 input/change(untrusted)
fn fill_fallback(app: &AppHandle, selector: &str, value: &str) -> ControlResult<()> {
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
    super::eval(app, &js)?;
    Ok(())
}

/// DOM 赋值 select + 派发 input/change(untrusted),返回读回值
fn select_fallback(app: &AppHandle, selector: &str, value: &str) -> ControlResult<String> {
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
    let v = super::eval(app, &js)?;
    check_error(&v)?;
    Ok(v.get("value").and_then(Value::as_str).unwrap_or("").to_string())
}

/// iframe 内 DOM 赋值 + 派发 input/change(untrusted)
fn fill_in_iframe_fallback(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
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
    super::eval(app, &js)?;
    Ok(())
}

// ---- Windows 可信实现 ----

/// 可信 fill:focus → 全选 → insertText → 读回校验
#[cfg(windows)]
fn fill_trusted(app: &AppHandle, selector: &str, value: &str) -> ControlResult<()> {
    keyboard::focus_element(app, selector)?;
    select_all(app, selector)?;
    if value.is_empty() {
        // 空值:用 Delete 清掉选中内容(insertText("") 可能为空操作)
        keyboard::press_key(app, "Delete", "")?;
    } else {
        insert_text(app, value)?;
    }
    verify_value(app, selector, value)
}

/// 可信 clear:focus → 全选 → Delete → 读回校验
#[cfg(windows)]
fn clear_trusted(app: &AppHandle, selector: &str) -> ControlResult<()> {
    keyboard::focus_element(app, selector)?;
    select_all(app, selector)?;
    keyboard::press_key(app, "Delete", "")?;
    verify_value(app, selector, "")
}

/// 可信 select:仅单值 select 适用;返回 Some(实际值);`multiple`/`size>1` 返回 None 交给降级
#[cfg(windows)]
fn select_trusted_if_supported(
    app: &AppHandle,
    selector: &str,
    value: &str,
) -> ControlResult<Option<String>> {
    let info = select_info(app, selector, value)?;
    if info.multiple || info.size > 1 {
        return Ok(None);
    }
    if info.target_index < 0 {
        return Err(format!("option not found: {value}"));
    }
    keyboard::focus_element(app, selector)?;
    let delta = info.target_index - info.selected_index;
    let key = if delta >= 0 { "ArrowDown" } else { "ArrowUp" };
    for _ in 0..delta.abs() {
        keyboard::press_key(app, key, "")?;
    }
    let actual = read_value(app, selector)?;
    if actual != value {
        return Err(format!(
            "select verification failed: expected {value:?}, got {actual:?} \
             (trusted keyboard path did not change selection)"
        ));
    }
    Ok(Some(actual))
}

/// 可信 iframe_fill:iframe 内 focus → 全选 → insertText → 读回
#[cfg(windows)]
fn fill_in_iframe_trusted(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
    value: &str,
) -> ControlResult<()> {
    focus_in_iframe(app, iframe_selector, selector)?;
    select_all_in_iframe(app, iframe_selector, selector)?;
    if value.is_empty() {
        keyboard::press_key(app, "Delete", "")?;
    } else {
        insert_text(app, value)?;
    }
    let actual = read_value_in_iframe(app, iframe_selector, selector)?;
    if actual != value {
        return Err(format!(
            "iframe fill verification failed: expected {value:?}, got {actual:?}"
        ));
    }
    Ok(())
}

/// 发送 CDP `Input.insertText`(在当前聚焦元素处插入/替换选区)
#[cfg(windows)]
fn insert_text(app: &AppHandle, text: &str) -> ControlResult<()> {
    let params = serde_json::json!({ "text": text }).to_string();
    cdp::call_json(app, "Input.insertText", &params)?;
    Ok(())
}

/// 选中目标元素全部内容(input/textarea 用 select()/setSelectionRange,contenteditable 用 Range)
#[cfg(windows)]
fn select_all(app: &AppHandle, selector: &str) -> ControlResult<()> {
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
    super::eval(app, &js)?;
    Ok(())
}

/// 读取元素当前值(input/textarea 取 value,contenteditable 取 textContent)
#[cfg(windows)]
fn read_value(app: &AppHandle, selector: &str) -> ControlResult<String> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          return {{value: (typeof el.value === "string") ? el.value : (el.textContent || "")}};
        }})()"#,
        sel = selector
    );
    let v = super::eval(app, &js)?;
    check_error(&v)?;
    Ok(v.get("value").and_then(Value::as_str).unwrap_or("").to_string())
}

/// 读回校验:实际值必须等于期望值,否则报错(不静默成功)
#[cfg(windows)]
fn verify_value(app: &AppHandle, selector: &str, expected: &str) -> ControlResult<()> {
    let actual = read_value(app, selector)?;
    if actual != expected {
        return Err(format!(
            "fill verification failed: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

/// select 元信息(供可信键盘路径判断是否适用)
#[cfg(windows)]
struct SelectInfo {
    multiple: bool,
    size: i64,
    selected_index: i64,
    target_index: i64,
}

/// 查询 select 的 multiple/size/当前索引/目标值所在索引;未找到目标值 target_index = -1
#[cfg(windows)]
fn select_info(app: &AppHandle, selector: &str, value: &str) -> ControlResult<SelectInfo> {
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
    let v = super::eval(app, &js)?;
    check_error(&v)?;
    Ok(SelectInfo {
        multiple: v.get("multiple").and_then(Value::as_bool).unwrap_or(false),
        size: v.get("size").and_then(Value::as_i64).unwrap_or(0),
        selected_index: v.get("selectedIndex").and_then(Value::as_i64).unwrap_or(0),
        target_index: v.get("targetIndex").and_then(Value::as_i64).unwrap_or(-1),
    })
}

/// iframe 内容文档内聚焦元素
#[cfg(windows)]
fn focus_in_iframe(app: &AppHandle, iframe_selector: &str, selector: &str) -> ControlResult<()> {
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
    let v = super::eval(app, &js)?;
    check_error(&v)?;
    if !v.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Err(format!("element is not focusable in iframe: {selector}"));
    }
    Ok(())
}

/// iframe 内容文档内全选
#[cfg(windows)]
fn select_all_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
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
    super::eval(app, &js)?;
    Ok(())
}

/// iframe 内容文档内读取 value/textContent
#[cfg(windows)]
fn read_value_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
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
    let v = super::eval(app, &js)?;
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
