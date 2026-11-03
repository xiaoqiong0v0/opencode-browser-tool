//! 页面脚本/取值公共层：可视中心、滚动容器、视口尺寸与按 label 的 eval。
//!
//! 拖拽（鼠标/触摸）共用这里的唯一实现，避免多份 `getBoundingClientRect` 逻辑分叉。

use serde_json::Value;
use tauri::AppHandle;

use super::ControlResult;

/// 取页面视口尺寸
pub(crate) fn viewport_script() -> &'static str {
    "(function(){return {width: window.innerWidth || 0, height: window.innerHeight || 0};})()"
}

/// 元素可视中心 JS：元素矩形 ∩ 所有可滚动祖先 clientRect ∩ 视口。
///
/// 非法 selector / 运行时异常都在 JS 内 try/catch，返回 `{error: "..."}`，避免 eval 挂起。
pub(crate) fn center_script(selector: &str) -> String {
    format!(
        r#"(function(){{
          try {{
            var el;
            try {{ el = document.querySelector({sel:?}); }}
            catch (e) {{ return {{error: "invalid selector: " + ((e && e.message) || e)}}; }}
            if (!el) return {{error: "not found"}};
            var r = el.getBoundingClientRect();
            var vw = window.innerWidth || 0, vh = window.innerHeight || 0;
            var left = Math.max(0, r.left), top = Math.max(0, r.top);
            var right = Math.min(vw, r.right), bottom = Math.min(vh, r.bottom);
            var p = el.parentElement;
            while (p) {{
              var cs = getComputedStyle(p);
              var sx = cs.overflowX !== "visible" && p.scrollWidth > p.clientWidth;
              var sy = cs.overflowY !== "visible" && p.scrollHeight > p.clientHeight;
              if (sx || sy) {{
                var pr = p.getBoundingClientRect();
                var bl = parseFloat(cs.borderLeftWidth) || 0;
                var bt = parseFloat(cs.borderTopWidth) || 0;
                var br = parseFloat(cs.borderRightWidth) || 0;
                var bb = parseFloat(cs.borderBottomWidth) || 0;
                left = Math.max(left, pr.left + bl);
                top = Math.max(top, pr.top + bt);
                right = Math.min(right, pr.right - br);
                bottom = Math.min(bottom, pr.bottom - bb);
              }}
              p = p.parentElement;
            }}
            if (!(right > left && bottom > top)) return {{error: "center not visible"}};
            return {{x: left + (right - left) / 2, y: top + (bottom - top) / 2}};
          }} catch (e) {{
            return {{error: "eval failed: " + ((e && e.message) || e)}};
          }}
        }})()"#,
        sel = selector
    )
}

/// 调整容器滚动位置的 JS
pub(crate) fn scroll_container_script(selector: &str, dx: f64, dy: f64) -> String {
    format!(
        r#"(function(){{
          try {{
            var el;
            try {{ el = document.querySelector({sel:?}); }}
            catch (e) {{ return {{error: "invalid selector: " + ((e && e.message) || e)}}; }}
            if (!el) return {{error: "not found"}};
            el.scrollLeft += {dx};
            el.scrollTop += {dy};
            return {{ok: true}};
          }} catch (e) {{
            return {{error: "eval failed: " + ((e && e.message) || e)}};
          }}
        }})()"#,
        sel = selector
    )
}

/// 定向到固定 page label 执行 JS 并解析 JSON（Windows）
#[cfg(windows)]
pub(crate) fn eval_label_value(
    app: &AppHandle,
    label: &str,
    js: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Value> {
    let budget = timeout_ms.unwrap_or(super::cdp::CDP_TIMEOUT_MS);
    let raw = crate::ui::eval_page_label_timeout(app, label, js, budget)?;
    serde_json::from_str(&raw).map_err(|e| format!("parse eval result: {e}"))
}
