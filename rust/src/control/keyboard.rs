//! 键盘注入:平台可信通道(产生 isTrusted=true 的事件并触发浏览器默认行为)
//!
//! Windows:CDP `Input.dispatchKeyEvent`(复用 `control::cdp`),先聚焦目标元素再注入
//! Linux(WebKitGTK):可信路径尚未确证(见仓库报告),暂不提供,返回明确错误而非假成功
//! macOS 及其他平台:未实现,维持原有 JS 合成事件实现(不在本次改动范围)
//!
//! 契约:`--key <键名>` + 可选 `--selector <选择器>`。`key` 支持可打印单字符、
//! Enter/Tab/Escape/Backspace/Delete/方向键/Home/End/PageUp/PageDown/F1-F12/Space;
//! 组合键(Ctrl/Shift/Alt/Meta+字母)当前命令契约不支持,行为维持原状(仅单个按键)。

use tauri::AppHandle;

use super::ControlResult;

#[cfg(windows)]
use super::cdp;

/// CDP 修饰键位掩码(当前仅单键,恒为 0;保留以备后续扩展组合键)
#[allow(dead_code)]
const MOD_ALT: i64 = 1;
#[allow(dead_code)]
const MOD_CTRL: i64 = 2;
#[allow(dead_code)]
const MOD_META: i64 = 4;
#[allow(dead_code)]
const MOD_SHIFT: i64 = 8;

/// 单个按键的 CDP 描述
#[cfg(windows)]
struct KeyDef {
    /// DOM `KeyboardEvent.key` 值
    key: String,
    /// DOM `KeyboardEvent.code` 值
    code: String,
    /// Windows 虚拟键码(同时用作 nativeVirtualKeyCode)
    vk: i64,
    /// 可打印/控制字符文本;None 表示无文本(方向键、功能键等,走 rawKeyDown)
    text: Option<String>,
}

/// 注入一次单键(平台实现)
///
/// 参数:`app` 应用句柄;`key` 键名(如 `"a"`/`"Enter"`/`"ArrowUp"`/`"F5"`);
/// `selector` 目标元素选择器,空串表示注入到当前活动元素
/// 返回:成功 Ok(()),否则 Err(键不支持 / 元素不存在或不可聚焦 / 平台未实现 / CDP 失败)
#[cfg(windows)]
pub fn press_key(app: &AppHandle, key: &str, selector: &str) -> ControlResult<()> {
    if selector.is_empty() {
        // 无 selector:交给当前活动元素(与旧行为一致)
    } else {
        focus_element(app, selector)?;
    }
    let def = resolve_key(key).ok_or_else(|| format!("unsupported key: {key}"))?;
    // 有 text:keyDown 会生成 char 并触发默认行为;无 text:rawKeyDown
    let down_type = if def.text.is_some() { "keyDown" } else { "rawKeyDown" };
    dispatch(app, down_type, &def, true)?;
    dispatch(app, "keyUp", &def, false)?;
    Ok(())
}

/// 非 Windows 平台:可信键盘注入未实现,返回明确错误(避免旧的 JS 合成事件假成功)
#[cfg(target_os = "linux")]
pub fn press_key(_app: &AppHandle, _key: &str, _selector: &str) -> ControlResult<()> {
    Err("press_key: trusted keyboard injection is not implemented on this platform yet \
         (Windows via CDP; Linux WebKitGTK path unverified)"
        .into())
}

/// macOS 及其他平台:未实现,维持原有 JS 合成事件实现(本次改动范围外,不改行为)
///
/// 注意:该路径派发的是 unTrusted 的合成事件,不触发默认行为;仅为保持 macOS 现状。
#[cfg(not(any(windows, target_os = "linux")))]
pub fn press_key(app: &AppHandle, key: &str, _selector: &str) -> ControlResult<()> {
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
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, c)| c.to_string())
        .unwrap_or_else(|| key.to_string());
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
    super::eval(app, &js)?;
    Ok(())
}

/// 聚焦目标元素,确保后续 CDP 注入落到该元素而非 body
///
/// 元素不存在或 `focus()` 未生效(不可聚焦)时返回错误,避免静默落到错误目标
#[cfg(windows)]
fn focus_element(app: &AppHandle, selector: &str) -> ControlResult<()> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          if (el.scrollIntoView) el.scrollIntoView({{block: "center"}});
          if (el.focus) el.focus();
          return {{ok: document.activeElement === el}};
        }})()"#,
        sel = selector
    );
    let v = super::eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    if !v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) {
        return Err(format!(
            "element is not focusable: {selector} (focus() did not take; add tabindex or use a focusable element)"
        ));
    }
    Ok(())
}

/// 解析键名为 CDP 描述:先查命名键(含 F1-F12),再退化为单字符
#[cfg(windows)]
fn resolve_key(key: &str) -> Option<KeyDef> {
    if let Some(def) = named_key_def(key) {
        return Some(def);
    }
    if let Some(def) = function_key_def(key) {
        return Some(def);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(char_key_def(c)),
        _ => None,
    }
}

/// 命名键(非单字符)查表;大小写不敏感
#[cfg(windows)]
fn named_key_def(key: &str) -> Option<KeyDef> {
    let lower = key.to_ascii_lowercase();
    // (DOM key, DOM code, Windows VK, 文本)
    let (k, c, vk, text): (&str, &str, i64, Option<&str>) = match lower.as_str() {
        "enter" | "return" => ("Enter", "Enter", 13, Some("\r")),
        "tab" => ("Tab", "Tab", 9, Some("\t")),
        "escape" | "esc" => ("Escape", "Escape", 27, Some("\u{1b}")),
        "backspace" => ("Backspace", "Backspace", 8, Some("\u{8}")),
        "delete" | "del" => ("Delete", "Delete", 46, None),
        "arrowup" | "up" => ("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => ("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => ("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => ("ArrowRight", "ArrowRight", 39, None),
        "home" => ("Home", "Home", 36, None),
        "end" => ("End", "End", 35, None),
        "pageup" => ("PageUp", "PageUp", 33, None),
        "pagedown" => ("PageDown", "PageDown", 34, None),
        "space" | " " => (" ", "Space", 32, Some(" ")),
        _ => return None,
    };
    Some(KeyDef {
        key: k.to_string(),
        code: c.to_string(),
        vk,
        text: text.map(str::to_string),
    })
}

/// F1-F12:VK 从 112(F1)连续递增
#[cfg(windows)]
fn function_key_def(key: &str) -> Option<KeyDef> {
    let n: u32 = key.to_ascii_lowercase().strip_prefix('f')?.parse().ok()?;
    if !(1..=12).contains(&n) {
        return None;
    }
    Some(KeyDef {
        key: format!("F{n}"),
        code: format!("F{n}"),
        vk: 111 + i64::from(n),
        text: None,
    })
}

/// 单可打印字符:key/text 即该字符;code/VK 按 ASCII 规则推导
#[cfg(windows)]
fn char_key_def(c: char) -> KeyDef {
    let (code, vk) = char_code_vk(c);
    KeyDef {
        key: c.to_string(),
        code,
        vk,
        text: Some(c.to_string()),
    }
}

/// 单字符的 DOM code 与 Windows VK(未知标点用空 code / VK 0,插入依赖 text)
#[cfg(windows)]
fn char_code_vk(c: char) -> (String, i64) {
    if c.is_ascii_alphabetic() {
        let up = c.to_ascii_uppercase();
        return (format!("Key{up}"), up as i64);
    }
    if c.is_ascii_digit() {
        return (format!("Digit{c}"), c as i64);
    }
    // 未按 Shift 的常见标点
    let (code, vk) = match c {
        '-' => ("Minus", 189),
        '=' => ("Equal", 187),
        '[' => ("BracketLeft", 219),
        ']' => ("BracketRight", 221),
        '\\' => ("Backslash", 220),
        ';' => ("Semicolon", 186),
        '\'' => ("Quote", 222),
        ',' => ("Comma", 188),
        '.' => ("Period", 190),
        '/' => ("Slash", 191),
        '`' => ("Backquote", 192),
        _ => ("", 0),
    };
    (code.to_string(), vk)
}

/// 发送一次 CDP `Input.dispatchKeyEvent`
///
/// 参数:`event_type` 为 `"keyDown"`/`"rawKeyDown"`/`"keyUp"`;`with_text` 为 true 时附带
/// `text`/`unmodifiedText`(仅按下阶段需要,抬起阶段不带以免重复插入)
#[cfg(windows)]
fn dispatch(app: &AppHandle, event_type: &str, def: &KeyDef, with_text: bool) -> ControlResult<()> {
    let mut params = serde_json::json!({
        "type": event_type,
        "key": def.key,
        "code": def.code,
        "windowsVirtualKeyCode": def.vk,
        "nativeVirtualKeyCode": def.vk,
        "modifiers": 0,
    });
    if with_text {
        if let Some(text) = &def.text {
            params["text"] = serde_json::Value::String(text.clone());
            params["unmodifiedText"] = serde_json::Value::String(text.clone());
        }
    }
    let params =
        serde_json::to_string(&params).map_err(|e| format!("serialize key params: {e}"))?;
    cdp::call_json(app, "Input.dispatchKeyEvent", &params)?;
    Ok(())
}
