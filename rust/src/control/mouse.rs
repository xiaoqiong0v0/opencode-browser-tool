//! 可信鼠标注入:点击 / 悬停 / iframe 内点击
//!
//! Windows:CDP `Input.dispatchMouseEvent`(与真实鼠标一致,产生 isTrusted 事件、真实命中测试、
//!   触发 CSS `:hover` 与元素默认行为)。公用发送函数 `mouse_event` 由点击/悬停/拖拽共用。
//! Linux/其他:未实现,返回明确错误(旧 `el.click()` / 合成 MouseEvent 为 untrusted,不在本平台回退)
//!
//! 命中校验:注入前用 `document.elementFromPoint` 在目标中心做命中测试;
//!   中心不在视口内、或被其它元素遮挡时返回明确错误,避免"点到了别处却报成功"。

use tauri::AppHandle;

use super::ControlResult;

#[cfg(windows)]
use serde_json::Value;

/// 视口坐标点(CSS 像素)
#[derive(Clone, Copy, Debug)]
pub struct Point {
    /// 视口 X(CSS 像素)
    pub x: f64,
    /// 视口 Y(CSS 像素)
    pub y: f64,
}

/// 发送一次 CDP `Input.dispatchMouseEvent`(Windows;点击/悬停/拖拽共用)
///
/// 参数:`event_type` ∈ mouseMoved/mousePressed/mouseReleased 等;`x`/`y` 视口 CSS 像素;
/// `button` ∈ none/left/right/middle;`buttons` 按下位掩码(left=1);`click_count` 连击数
#[cfg(windows)]
pub fn mouse_event(
    app: &AppHandle,
    event_type: &str,
    x: f64,
    y: f64,
    button: &str,
    buttons: i64,
    click_count: i64,
) -> ControlResult<()> {
    let params = serde_json::json!({
        "type": event_type,
        "x": x,
        "y": y,
        "button": button,
        "buttons": buttons,
        "clickCount": click_count,
    })
    .to_string();
    super::cdp::call_json(app, "Input.dispatchMouseEvent", &params)?;
    Ok(())
}

/// 可信点击元素中心(Windows):mouseMoved → mousePressed → mouseReleased
///
/// 返回实际注入的视口坐标;中心出视口 / 被遮挡 / 元素不存在时返回 Err
pub fn click(app: &AppHandle, selector: &str) -> ControlResult<Point> {
    #[cfg(windows)]
    {
        click_at(app, None, selector)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, selector);
        Err("click: trusted mouse injection is not implemented on this platform yet \
             (Windows via CDP Input.dispatchMouseEvent; Linux/WebKitGTK unverified)"
            .into())
    }
}

/// 可信悬停元素中心(Windows):仅 mouseMoved(真实移动即触发 CSS `:hover`)
pub fn hover(app: &AppHandle, selector: &str) -> ControlResult<Point> {
    #[cfg(windows)]
    {
        let p = resolve_point(app, None, selector)?;
        ensure_reachable(&p, selector)?;
        mouse_event(app, "mouseMoved", p.x, p.y, "none", 0, 0)?;
        Ok(Point { x: p.x, y: p.y })
    }
    #[cfg(not(windows))]
    {
        let _ = (app, selector);
        Err("hover: trusted mouse injection is not implemented on this platform yet \
             (Windows via CDP Input.dispatchMouseEvent; Linux/WebKitGTK unverified)"
            .into())
    }
}

/// 可信点击 iframe 内元素中心(Windows):iframe 内容坐标换算到顶层视口后点击
///
/// 跨域 iframe(`contentDocument` 为 null)返回明确错误,不静默点到别处
pub fn click_in_iframe(
    app: &AppHandle,
    iframe_selector: &str,
    selector: &str,
) -> ControlResult<Point> {
    #[cfg(windows)]
    {
        click_at(app, Some(iframe_selector), selector)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, iframe_selector, selector);
        Err("iframe_click: trusted mouse injection is not implemented on this platform yet \
             (Windows via CDP Input.dispatchMouseEvent; Linux/WebKitGTK unverified)"
            .into())
    }
}

/// Windows:解析坐标 → 命中校验 → 注入鼠标按下/抬起
#[cfg(windows)]
fn click_at(
    app: &AppHandle,
    iframe_selector: Option<&str>,
    selector: &str,
) -> ControlResult<Point> {
    let p = resolve_point(app, iframe_selector, selector)?;
    ensure_reachable(&p, selector)?;
    ensure_enabled(&p, selector)?;
    mouse_event(app, "mouseMoved", p.x, p.y, "none", 0, 0)?;
    mouse_event(app, "mousePressed", p.x, p.y, "left", 1, 1)?;
    mouse_event(app, "mouseReleased", p.x, p.y, "left", 0, 1)?;
    Ok(Point { x: p.x, y: p.y })
}

/// 禁用元素校验:真实鼠标点击不会在 disabled 控件上派发 click,提前报错避免假成功
#[cfg(windows)]
fn ensure_enabled(p: &Resolved, selector: &str) -> ControlResult<()> {
    if p.disabled {
        return Err(format!(
            "element is disabled, click not dispatched: {selector}"
        ));
    }
    Ok(())
}

/// 中心可达性校验:必须在视口内,且命中测试确实落在目标(或其后代/祖先)上
#[cfg(windows)]
fn ensure_reachable(p: &Resolved, selector: &str) -> ControlResult<()> {
    if !p.in_viewport {
        return Err(format!(
            "element center is outside the viewport: {selector} (x={:.1}, y={:.1})",
            p.x, p.y
        ));
    }
    if !p.hit_ok {
        let hit = p.hit.as_deref().unwrap_or("(none)");
        return Err(format!(
            "element is covered at its center, mouse not dispatched: {selector} (hit: {hit})"
        ));
    }
    Ok(())
}

/// 命中/坐标解析结果
#[cfg(windows)]
struct Resolved {
    x: f64,
    y: f64,
    hit_ok: bool,
    hit: Option<String>,
    in_viewport: bool,
    disabled: bool,
}

/// 解析元素中心的顶层视口坐标,并做命中测试与视口判断
///
/// `iframe_selector` 为 Some 时,在 iframe 内容文档内查询元素,坐标加上 iframe 的
/// 边框/内边距偏移换算到顶层视口;命中测试在该内容文档内完成。
#[cfg(windows)]
fn resolve_point(
    app: &AppHandle,
    iframe_selector: Option<&str>,
    selector: &str,
) -> ControlResult<Resolved> {
    // iframe 分支:定位 iframe → 取内容文档 → 在内容文档内查询元素
    let lookup = match iframe_selector {
        Some(iframe) => format!(
            r#"
          f = document.querySelector({iframe:?});
          if (!f) return {{error: "iframe not found: " + {iframe:?}}};
          doc = f.contentDocument;
          if (!doc) return {{error: "cross-origin iframe not accessible: " + {iframe:?}}};
        "#,
            iframe = iframe
        ),
        None => String::new(),
    };
    let js = format!(
        r##"(function(){{
          var doc = document, f = null, offX = 0, offY = 0;
          {lookup}
          var el = doc.querySelector({sel:?});
          if (!el) return {{error: "element not found: " + {sel:?}}};
          if (el.scrollIntoView) el.scrollIntoView({{block: "center", inline: "center"}});
          if (f) {{
            if (f.scrollIntoView) f.scrollIntoView({{block: "center", inline: "center"}});
            var cs = getComputedStyle(f);
            var fr = f.getBoundingClientRect();
            offX = fr.left + (parseFloat(cs.borderLeftWidth) || 0) + (parseFloat(cs.paddingLeft) || 0);
            offY = fr.top + (parseFloat(cs.borderTopWidth) || 0) + (parseFloat(cs.paddingTop) || 0);
          }}
          var r = el.getBoundingClientRect();
          var ix = r.left + r.width / 2;
          var iy = r.top + r.height / 2;
          var x = offX + ix, y = offY + iy;
          var hitEl = doc.elementFromPoint(ix, iy);
          var hitOk = !!(hitEl && (hitEl === el || (el.contains && el.contains(hitEl)) || (hitEl.contains && hitEl.contains(el))));
          var hitName = hitEl ? (hitEl.id ? ("#" + hitEl.id) : hitEl.tagName.toLowerCase()) : null;
          var cw = doc.documentElement ? doc.documentElement.clientWidth : 0;
          var ch = doc.documentElement ? doc.documentElement.clientHeight : 0;
          var innerIn = ix >= 0 && iy >= 0 && ix < cw && iy < ch;
          var topIn = x >= 0 && y >= 0 && x < window.innerWidth && y < window.innerHeight;
          return {{x: x, y: y, hitOk: hitOk, hit: hitName, inViewport: innerIn && topIn, disabled: !!el.disabled}};
        }})()"##,
        lookup = lookup,
        sel = selector
    );
    let v = super::eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let x = v.get("x").and_then(Value::as_f64);
    let y = v.get("y").and_then(Value::as_f64);
    let (Some(x), Some(y)) = (x, y) else {
        return Err("mouse: failed to resolve element coordinates".into());
    };
    Ok(Resolved {
        x,
        y,
        hit_ok: v.get("hitOk").and_then(Value::as_bool).unwrap_or(false),
        hit: v.get("hit").and_then(Value::as_str).map(str::to_string),
        in_viewport: v.get("inViewport").and_then(Value::as_bool).unwrap_or(false),
        disabled: v.get("disabled").and_then(Value::as_bool).unwrap_or(false),
    })
}
