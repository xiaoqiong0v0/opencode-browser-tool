//! 拖拽:用能真正触发原生 HTML5 DnD 的通道
//!
//! Windows:CDP `Input.dispatchMouseEvent` 鼠标序列(mouseMoved → mousePressed → 多步 mouseMoved → mouseReleased),
//!   由 Chromium 拖拽控制器真正发起 dragstart/dragover/drop(与真实鼠标一致)。
//! Linux/其他:未实现,返回明确错误
//!   (旧 `dispatchEvent(new DragEvent(...))` 只调用页面 JS 监听器,不会触发浏览器内部 DnD 激活行为,且 DataTransfer 从无数据 → 必然假成功)
//!
//! 说明:HTML5 DnD 的 `dragstart` 里由页面 `dataTransfer.setData(...)` 建立数据;
//! 本通道只需真实鼠标序列,drop 侧即可拿到页面设置的数据。

use tauri::AppHandle;

use super::ControlResult;

#[cfg(windows)]
use super::mouse::mouse_event_with;
#[cfg(windows)]
use serde_json::Value;

/// 把 source 元素拖到 target 元素
///
/// 参数:`app` 应用句柄;`source_selector` / `target_selector` 源与目标选择器
/// 返回:成功 Ok(());元素不存在 / 平台未实现返回 Err
pub fn drag(app: &AppHandle, source_selector: &str, target_selector: &str) -> ControlResult<()> {
    drag_with(app, source_selector, target_selector, None)
}

/// 同 `drag`,`timeout_ms` 为 Some 时贯穿:取中心的 eval + 每次 CDP 鼠标事件都用该预算
/// 最坏耗时 ≈ 单步预算 × 步数(1 次 eval + 2 + DRAG_STEPS + 1 次 CDP)
#[cfg(windows)]
pub fn drag_with(
    app: &AppHandle,
    source_selector: &str,
    target_selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    drag_windows(app, source_selector, target_selector, timeout_ms)
}

/// 非 Windows:未实现(保留签名以便统一调用)
#[cfg(not(windows))]
pub fn drag_with(
    app: &AppHandle,
    source_selector: &str,
    target_selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let _ = (app, source_selector, target_selector, timeout_ms);
    Err("drag: not implemented on this platform yet \
         (Windows via CDP Input.dispatchMouseEvent; Linux/WebKitGTK unverified)"
        .into())
}

/// 鼠标序列的中间插值步数(足以越过拖拽阈值并触发 dragover)
#[cfg(windows)]
const DRAG_STEPS: i32 = 8;

/// Windows:先取源/目标中心坐标,再注入鼠标按下→移动→抬起序列
#[cfg(windows)]
fn drag_windows(
    app: &AppHandle,
    source_selector: &str,
    target_selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    let pt = element_centers(app, source_selector, target_selector, timeout_ms)?;
    let sx = pt.0;
    let sy = pt.1;
    let tx = pt.2;
    let ty = pt.3;

    // 移到起点(未按下)
    mouse_event_with(app, "mouseMoved", sx, sy, "none", 0, 0, timeout_ms)?;
    // 按下左键
    mouse_event_with(app, "mousePressed", sx, sy, "left", 1, 1, timeout_ms)?;
    // 分步移动到目标(按住左键),越过拖拽阈值并产生 dragover
    for i in 1..=DRAG_STEPS {
        let t = f64::from(i) / f64::from(DRAG_STEPS);
        let x = sx + (tx - sx) * t;
        let y = sy + (ty - sy) * t;
        mouse_event_with(app, "mouseMoved", x, y, "left", 1, 0, timeout_ms)?;
    }
    // 在目标处释放
    mouse_event_with(app, "mouseReleased", tx, ty, "left", 0, 1, timeout_ms)?;
    Ok(())
}

/// 取源/目标元素中心(视口 CSS 像素);元素不存在返回明确错误
#[cfg(windows)]
fn element_centers(
    app: &AppHandle,
    source_selector: &str,
    target_selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<(f64, f64, f64, f64)> {
    let js = format!(
        r#"(function(){{
          var s = document.querySelector({src:?});
          var d = document.querySelector({dst:?});
          if (!s) return {{error: "source element not found"}};
          if (!d) return {{error: "target element not found"}};
          if (s.scrollIntoView) s.scrollIntoView({{block: "center", inline: "center"}});
          if (d.scrollIntoView) d.scrollIntoView({{block: "center", inline: "center"}});
          var rs = s.getBoundingClientRect();
          var rd = d.getBoundingClientRect();
          return {{
            sx: rs.left + rs.width / 2, sy: rs.top + rs.height / 2,
            tx: rd.left + rd.width / 2, ty: rd.top + rd.height / 2
          }};
        }})()"#,
        src = source_selector,
        dst = target_selector
    );
    let v = super::eval_with(app, &js, timeout_ms)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let get = |key: &str| v.get(key).and_then(Value::as_f64);
    match (get("sx"), get("sy"), get("tx"), get("ty")) {
        (Some(sx), Some(sy), Some(tx), Some(ty)) => Ok((sx, sy, tx, ty)),
        _ => Err("drag: failed to read element centers".into()),
    }
}
