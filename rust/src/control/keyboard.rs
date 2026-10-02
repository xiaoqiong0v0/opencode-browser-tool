//! 键盘注入:平台可信通道(产生 isTrusted=true 的事件并触发浏览器默认行为)
//!
//! Windows:CDP `Input.dispatchKeyEvent`(复用 `control::cdp`),先聚焦目标元素再注入
//! Linux(WebKitGTK):GDK `gdk_event_put` 注入原生键盘事件(复用 `control::gdk`),已实测可信
//! macOS 及其他:无可信通道;旧 JS 只派发 untrusted keydown/keyup,不产生输入/默认行为,
//!   属确定性假成功 → 明确报错,不做降级兜底
//!
//! 契约:`--key <键名>` + 可选 `--selector <选择器>`。`key` 支持可打印单字符、
//! Enter/Tab/Escape/Backspace/Delete/方向键/Home/End/PageUp/PageDown/F1-F12/Space;
//! 组合键(Ctrl/Shift/Alt/Meta+字母)当前命令契约不支持,行为维持原状(仅单个按键)。

use tauri::AppHandle;

use super::ControlResult;

#[cfg(windows)]
use super::cdp;

/// 注入一次单键(平台实现)
///
/// 参数:`app` 应用句柄;`key` 键名(如 `"a"`/`"Enter"`/`"ArrowUp"`/`"F5"`);
/// `selector` 目标元素选择器,空串表示注入到当前活动元素
/// 返回:成功 Ok(()),否则 Err(键不支持 / 元素不存在或不可聚焦 / 平台无可信通道 / 注入失败)
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

/// Linux:先聚焦目标元素,再用 GDK 注入原生键盘事件
#[cfg(target_os = "linux")]
pub fn press_key(app: &AppHandle, key: &str, selector: &str) -> ControlResult<()> {
    if !selector.is_empty() {
        focus_element(app, selector)?;
    }
    let keyval = keyval_for(key).ok_or_else(|| format!("unsupported key: {key}"))?;
    super::gdk::inject(app, vec![super::gdk::Inj::Key(keyval)])
}

/// 非 Windows/Linux:无可信键盘通道 → 明确报错
#[cfg(not(any(windows, target_os = "linux")))]
pub fn press_key(_app: &AppHandle, _key: &str, _selector: &str) -> ControlResult<()> {
    Err("press_key: no trusted keyboard channel on this platform \
         (Windows via CDP, Linux via GDK; macOS unverified) — refusing untrusted JS fallback"
        .into())
}

/// 在当前聚焦元素处插入文本(可信文本输入通道;供 `fill` 复用)
///
/// Windows:CDP `Input.insertText`(替换选区);Linux:GDK 逐字符注入按键(已实测可输入大小写/数字/符号)
#[cfg(windows)]
pub fn insert_text(app: &AppHandle, text: &str) -> ControlResult<()> {
    let params = serde_json::json!({ "text": text }).to_string();
    cdp::call_json(app, "Input.insertText", &params)?;
    Ok(())
}

/// Linux:逐字符 GDK 注入
#[cfg(target_os = "linux")]
pub fn insert_text(app: &AppHandle, text: &str) -> ControlResult<()> {
    if text.is_empty() {
        return Ok(());
    }
    let events: Vec<super::gdk::Inj> = text
        .chars()
        .map(|c| super::gdk::Inj::Key(super::gdk::keyval_from_char(c)))
        .collect();
    super::gdk::inject(app, events)
}

/// 非 Windows/Linux:无可信文本通道
#[cfg(not(any(windows, target_os = "linux")))]
pub fn insert_text(_app: &AppHandle, _text: &str) -> ControlResult<()> {
    Err("insert_text: no trusted text channel on this platform".into())
}

/// 聚焦目标元素,确保后续注入落到该元素而非 body(供键盘/文本输入复用)
///
/// 元素不存在或 `focus()` 未生效(不可聚焦)时返回错误,避免静默落到错误目标
pub fn focus_element(app: &AppHandle, selector: &str) -> ControlResult<()> {
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

// ---- 键名解析(Windows/Linux 共用命名键表,平台各自映射为 VK 或 keyval) ----

/// 平台无关的命名键
#[cfg(any(windows, target_os = "linux"))]
#[derive(Debug, Clone, Copy)]
enum NamedKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    Func(u32),
}

/// 解析命名键名(大小写不敏感);单字符与未知键返回 None
#[cfg(any(windows, target_os = "linux"))]
fn resolve_named(key: &str) -> Option<NamedKey> {
    let lower = key.to_ascii_lowercase();
    Some(match lower.as_str() {
        "enter" | "return" => NamedKey::Enter,
        "tab" => NamedKey::Tab,
        "escape" | "esc" => NamedKey::Escape,
        "backspace" => NamedKey::Backspace,
        "delete" | "del" => NamedKey::Delete,
        "arrowup" | "up" => NamedKey::ArrowUp,
        "arrowdown" | "down" => NamedKey::ArrowDown,
        "arrowleft" | "left" => NamedKey::ArrowLeft,
        "arrowright" | "right" => NamedKey::ArrowRight,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" => NamedKey::PageUp,
        "pagedown" => NamedKey::PageDown,
        "space" | " " => NamedKey::Space,
        _ => {
            let n: u32 = lower.strip_prefix('f')?.parse().ok()?;
            if !(1..=12).contains(&n) {
                return None;
            }
            NamedKey::Func(n)
        }
    })
}

// ---- Windows:CDP 键描述与发送 ----

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

/// 解析键名为 CDP 描述:先查命名键(含 F1-F12),再退化为单字符
#[cfg(windows)]
fn resolve_key(key: &str) -> Option<KeyDef> {
    if let Some(def) = named_key_def(key) {
        return Some(def);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(char_key_def(c)),
        _ => None,
    }
}

/// 命名键 → CDP 描述;大小写不敏感
#[cfg(windows)]
fn named_key_def(key: &str) -> Option<KeyDef> {
    match resolve_named(key)? {
        NamedKey::Func(n) => Some(KeyDef {
            key: format!("F{n}"),
            code: format!("F{n}"),
            vk: 111 + i64::from(n),
            text: None,
        }),
        nk => {
            // (DOM key, DOM code, Windows VK, 文本)
            let (k, c, vk, text): (&str, &str, i64, Option<&str>) = match nk {
                NamedKey::Enter => ("Enter", "Enter", 13, Some("\r")),
                NamedKey::Tab => ("Tab", "Tab", 9, Some("\t")),
                NamedKey::Escape => ("Escape", "Escape", 27, Some("\u{1b}")),
                NamedKey::Backspace => ("Backspace", "Backspace", 8, Some("\u{8}")),
                NamedKey::Delete => ("Delete", "Delete", 46, None),
                NamedKey::ArrowUp => ("ArrowUp", "ArrowUp", 38, None),
                NamedKey::ArrowDown => ("ArrowDown", "ArrowDown", 40, None),
                NamedKey::ArrowLeft => ("ArrowLeft", "ArrowLeft", 37, None),
                NamedKey::ArrowRight => ("ArrowRight", "ArrowRight", 39, None),
                NamedKey::Home => ("Home", "Home", 36, None),
                NamedKey::End => ("End", "End", 35, None),
                NamedKey::PageUp => ("PageUp", "PageUp", 33, None),
                NamedKey::PageDown => ("PageDown", "PageDown", 34, None),
                NamedKey::Space => (" ", "Space", 32, Some(" ")),
                NamedKey::Func(_) => unreachable!(),
            };
            Some(KeyDef {
                key: k.to_string(),
                code: c.to_string(),
                vk,
                text: text.map(str::to_string),
            })
        }
    }
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

// ---- Linux:GDK keyval 映射 ----

/// 解析键名为 GDK keyval:先查命名键,再退化为单字符
#[cfg(target_os = "linux")]
fn keyval_for(key: &str) -> Option<u32> {
    use gtk::gdk::keys::constants as key;
    if let Some(nk) = resolve_named(key) {
        return match nk {
            NamedKey::Func(n) => function_keyval(n),
            NamedKey::Enter => Some(*key::Return),
            NamedKey::Tab => Some(*key::Tab),
            NamedKey::Escape => Some(*key::Escape),
            NamedKey::Backspace => Some(*key::BackSpace),
            NamedKey::Delete => Some(*key::Delete),
            NamedKey::ArrowUp => Some(*key::Up),
            NamedKey::ArrowDown => Some(*key::Down),
            NamedKey::ArrowLeft => Some(*key::Left),
            NamedKey::ArrowRight => Some(*key::Right),
            NamedKey::Home => Some(*key::Home),
            NamedKey::End => Some(*key::End),
            NamedKey::PageUp => Some(*key::Page_Up),
            NamedKey::PageDown => Some(*key::Page_Down),
            NamedKey::Space => Some(*key::space),
        };
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(super::gdk::keyval_from_char(c)),
        _ => None,
    }
}

/// F1-F12 的 GDK keyval
#[cfg(target_os = "linux")]
fn function_keyval(n: u32) -> Option<u32> {
    use gtk::gdk::keys::constants as key;
    Some(match n {
        1 => *key::F1,
        2 => *key::F2,
        3 => *key::F3,
        4 => *key::F4,
        5 => *key::F5,
        6 => *key::F6,
        7 => *key::F7,
        8 => *key::F8,
        9 => *key::F9,
        10 => *key::F10,
        11 => *key::F11,
        12 => *key::F12,
        _ => return None,
    })
}
