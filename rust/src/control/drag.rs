//! 拖拽:Windows 使用拦截式原生 DnD 通道
//!
//! 流程:拦截开启 → 鼠标按下 + 小位移触发浏览器拖拽 → 等待 `Input.dragIntercepted` 的 dragData
//! → 后续移动派发 `dragEnter`/`dragOver` → 结束派发 `drop`。
//! 不再依赖纯鼠标序列(实测 WebView2 上不会触发 `dragstart`)。
//! Linux/其他:未实现,返回明确错误。
//!
//! 1.14.0 起:
//! - 非批次拖拽使用「可视中心」= 元素矩形 ∩ 所有可滚动祖先的 clientRect ∩ 视口 的交集中心;
//!   不再自动 `scrollIntoView`(连续滚源/目标会互相把对方滚走),交集为空直接报错且不发鼠标事件。
//! - 新增 `--actions` 动作批次(单次调用内顺序执行,无跨命令状态),支持 move/press/scroll/wait/release,
//!   执行器用 RAII 清理保证拦截关闭与按键释放。
//! - 拖拽会话固定启动时的 page webview,切 tab 后所有 CDP/eval/鼠标事件仍定向原页。
//! - 除选择器外还支持视口坐标(`sourcePoint` / `targetPoint` / 动作里的 `to`)。

#[cfg(test)]
use std::collections::HashMap;
#[cfg(windows)]
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tauri::AppHandle;

use super::mouse::Point;
use super::page::{center_script, scroll_container_script, viewport_script};
use super::ControlResult;
#[cfg(windows)]
use super::page::eval_label_value;

/// 非批次拖拽的中间插值步数(足以越过拖拽阈值并触发 dragover)
#[cfg(windows)]
const DRAG_STEPS: usize = 8;
/// 批次 move 未指定 `steps` 时的默认插值步数(与非批次拖拽一致)
const DEFAULT_MOVE_STEPS: u32 = 8;
/// `steps` 上限:避免异常输入把单次调用拖成不可控长任务
const MAX_MOVE_STEPS: u32 = 1000;
/// 注入 wheel 后等待布局稳定的时间(ms)
const SCROLL_SETTLE_MS: u64 = 40;
/// 非 Windows 平台的统一错误
#[cfg(not(windows))]
const UNSUPPORTED: &str = "drag: unsupported platform (Windows only)";

/// 输入指针类型(mouse 默认;touch 为触摸通道)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pointer {
    /// 鼠标/原生 DnD
    Mouse,
    /// 触摸(touchStart/touchMove/touchEnd)
    Touch,
}

impl Pointer {
    /// 解析 `--pointer`(缺省 mouse;非法值明确报错)
    pub fn parse(text: &str) -> ControlResult<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "mouse" => Ok(Pointer::Mouse),
            "touch" => Ok(Pointer::Touch),
            other => Err(format!(
                "invalid pointer \"{other}\" (expected mouse|touch)"
            )),
        }
    }
}

/// 拖拽目标:选择器(取其可视中心)或视口 CSS 像素坐标
#[derive(Debug, Clone, PartialEq)]
pub enum DragTarget {
    /// CSS 选择器
    Selector(String),
    /// 视口坐标(CSS 像素)
    Point(f64, f64),
}

/// 批次动作(仅允许 move/press/scroll/wait/release)
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// 按住左键移动到目标;`steps` 为插值步数,`delay_ms` 为步间等待
    Move {
        target: DragTarget,
        steps: u32,
        delay_ms: u64,
    },
    /// 按下左键
    Press { target: DragTarget },
    /// 按下状态下注入 wheel;带 selector 时同时调整该容器的 scrollLeft/scrollTop
    Scroll {
        selector: Option<String>,
        dx: f64,
        dy: f64,
    },
    /// 等待(ms)
    Wait { ms: u64 },
    /// 抬起左键;`None` = 在当前位置抬起(通常由执行器 finally 自动补)
    Release { target: Option<DragTarget> },
}

/// 视口尺寸(CSS 像素)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// 宽
    pub width: f64,
    /// 高
    pub height: f64,
}

/// 拖拽完成信息(用于 `drag ok (steps=n, 耗时)`)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragDone {
    /// 执行的动作数(非批次 = 插值步数)
    pub steps: usize,
    /// 总耗时(ms)
    pub elapsed_ms: u64,
}

/// 解析 `"x,y"` 为视口坐标;格式错误 / 非有限数直接报错
pub fn parse_point(text: &str) -> Result<(f64, f64), String> {
    let mut parts = text.split(',');
    let raw_x = parts.next().unwrap_or("");
    let raw_y = parts.next().unwrap_or("");
    if parts.next().is_some() || raw_x.trim().is_empty() || raw_y.trim().is_empty() {
        return Err(format!(
            "drag: invalid point \"{text}\" (expected \"x,y\")"
        ));
    }
    let parse = |v: &str| {
        v.trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("drag: invalid point \"{text}\" (expected \"x,y\")"))
    };
    Ok((parse(raw_x)?, parse(raw_y)?))
}

/// 点是否在视口内(press 校验:`0 <= x < width && 0 <= y < height`)
pub fn point_in_viewport(p: Point, viewport: Viewport) -> bool {
    p.x >= 0.0 && p.x < viewport.width && p.y >= 0.0 && p.y < viewport.height
}

/// 解析批次 JSON 数组;语法/字段/状态机不合法都返回明确错误
pub fn parse_actions(json: &str) -> ControlResult<Vec<Action>> {
    let value: Value = serde_json::from_str(json)
        .map_err(|e| format!("drag: actions must be a JSON array: {e}"))?;
    let items = value
        .as_array()
        .ok_or("drag: actions must be a JSON array")?;
    if items.is_empty() {
        return Err("drag: actions must not be empty".into());
    }
    let mut actions = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        actions.push(parse_action(item).map_err(|e| format!("drag: action #{}: {e}", i + 1))?);
    }
    validate_actions(&actions)?;
    Ok(actions)
}

/// 解析单个动作对象
fn parse_action(value: &Value) -> ControlResult<Action> {
    let obj = value.as_object().ok_or("expected an object")?;
    if obj.len() != 1 {
        let keys = obj.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "expected exactly one action key (move/press/scroll/wait/release), got [{keys}]"
        ));
    }
    let (name, params) = obj.iter().next().ok_or("expected an action object")?;
    match name.as_str() {
        "move" => parse_move(params, name),
        "press" => {
            let obj = params.as_object().ok_or("expected an object")?;
            ensure_allowed(obj, &["selector", "to"], name)?;
            let target = parse_target(params, name, false)?.ok_or("selector or to is required")?;
            Ok(Action::Press { target })
        }
        "scroll" => parse_scroll(params, name),
        "wait" => {
            let obj = params.as_object().ok_or("expected an object")?;
            ensure_allowed(obj, &["ms"], name)?;
            Ok(Action::Wait {
                ms: required_u64(obj, "ms", name)?,
            })
        }
        "release" => {
            let target = match params {
                Value::Null => None,
                _ => {
                    let obj = params.as_object().ok_or("expected an object")?;
                    ensure_allowed(obj, &["selector", "to"], name)?;
                    parse_target(params, name, true)?
                }
            };
            Ok(Action::Release { target })
        }
        other => Err(format!(
            "unsupported action \"{other}\" (allowed: move, press, scroll, wait, release)"
        )),
    }
}

/// 解析 move 动作
fn parse_move(params: &Value, name: &str) -> ControlResult<Action> {
    let obj = params.as_object().ok_or("expected an object")?;
    ensure_allowed(obj, &["selector", "to", "steps", "delay"], name)?;
    let target = parse_target(params, name, false)?.ok_or("selector or to is required")?;
    let steps = match obj.get("steps") {
        Some(_) => required_u64(obj, "steps", name)?,
        None => DEFAULT_MOVE_STEPS as u64,
    };
    if steps == 0 || steps > u64::from(MAX_MOVE_STEPS) {
        return Err(format!(
            "steps must be 1..={MAX_MOVE_STEPS}, got: {steps}"
        ));
    }
    let delay_ms = match obj.get("delay") {
        Some(_) => required_u64(obj, "delay", name)?,
        None => 0,
    };
    Ok(Action::Move {
        target,
        steps: steps as u32,
        delay_ms,
    })
}

/// 解析 scroll 动作
fn parse_scroll(params: &Value, name: &str) -> ControlResult<Action> {
    let obj = params.as_object().ok_or("expected an object")?;
    ensure_allowed(obj, &["selector", "dx", "dy"], name)?;
    let selector = match obj.get("selector") {
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return Err("selector must not be empty".into());
            }
            super::validate_selector(s).map_err(|e| format!("selector: {e}"))?;
            Some(s.to_string())
        }
        Some(_) => return Err("selector must be a string".into()),
        None => None,
    };
    let dx = optional_f64(obj, "dx", name)?;
    let dy = optional_f64(obj, "dy", name)?;
    Ok(Action::Scroll { selector, dx, dy })
}

/// 解析 `selector` / `to` 二选一;`required=false` 时允许两者都缺省
fn parse_target(params: &Value, name: &str, optional: bool) -> ControlResult<Option<DragTarget>> {
    let obj = params.as_object().ok_or("expected an object")?;
    let selector = match obj.get("selector") {
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return Err("selector must not be empty".into());
            }
            super::validate_selector(s).map_err(|e| format!("selector: {e}"))?;
            Some(s.to_string())
        }
        Some(_) => return Err("selector must be a string".into()),
        None => None,
    };
    let point = match obj.get("to") {
        Some(Value::String(s)) => Some(parse_point(s)?),
        Some(_) => return Err("\"to\" must be a string \"x,y\"".into()),
        None => None,
    };
    match (selector, point) {
        (Some(_), Some(_)) => Err("selector and to are mutually exclusive".into()),
        (Some(s), None) => Ok(Some(DragTarget::Selector(s))),
        (None, Some((x, y))) => Ok(Some(DragTarget::Point(x, y))),
        (None, None) if optional => Ok(None),
        (None, None) => Err(format!("{name}: selector or to is required")),
    }
}

/// 校验动作状态机:press 前不能 move/scroll/release,release 后不能再有动作
pub fn validate_actions(actions: &[Action]) -> ControlResult<()> {
    let mut pressed = false;
    let mut released = false;
    for (i, action) in actions.iter().enumerate() {
        let n = i + 1;
        if released {
            return Err(format!(
                "drag: action #{n} follows release (release must be last)"
            ));
        }
        match action {
            Action::Wait { .. } => {}
            Action::Press { .. } => {
                if pressed {
                    return Err(format!("drag: action #{n} presses while already pressed"));
                }
                pressed = true;
            }
            Action::Move { .. } | Action::Scroll { .. } | Action::Release { .. } => {
                if !pressed {
                    return Err(format!(
                        "drag: action #{n} ({}) before press",
                        action_name(action)
                    ));
                }
            }
        }
        if matches!(action, Action::Release { .. }) {
            pressed = false;
            released = true;
        }
    }
    Ok(())
}

/// 动作名(错误信息用)
fn action_name(action: &Action) -> &'static str {
    match action {
        Action::Move { .. } => "move",
        Action::Press { .. } => "press",
        Action::Scroll { .. } => "scroll",
        Action::Wait { .. } => "wait",
        Action::Release { .. } => "release",
    }
}

/// 拒绝未声明的字段(把 `click`/`type` 等混入项挡在解析阶段)
fn ensure_allowed(obj: &Map<String, Value>, allowed: &[&str], name: &str) -> ControlResult<()> {
    for key in obj.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "{name}: unsupported field \"{key}\" (allowed: {})",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// 读取必填无符号整数
fn required_u64(obj: &Map<String, Value>, key: &str, name: &str) -> ControlResult<u64> {
    obj.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{name}: \"{key}\" must be a non-negative integer"))
}

/// 读取可选数字(缺省 0)
fn optional_f64(obj: &Map<String, Value>, key: &str, name: &str) -> ControlResult<f64> {
    match obj.get(key) {
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("{name}: \"{key}\" must be a finite number")),
        None => Ok(0.0),
    }
}

/// 鼠标/页面宿主抽象(便于纯逻辑单测;真实实现见 `WindowsHost`)
pub(crate) trait DragHost {
    /// 移动鼠标(未按下;按下后请用 `drag_to`)
    fn move_to(&mut self, p: Point) -> ControlResult<()>;
    /// 按下左键并启动拦截式拖拽(未收到 `Input.dragIntercepted` 即报错)
    fn press(&mut self, p: Point) -> ControlResult<()>;
    /// 拖拽态移动(拦截模式下派发 dragEnter/dragOver)
    fn drag_to(&mut self, p: Point) -> ControlResult<()>;
    /// 注入滚轮事件
    fn wheel(&mut self, p: Point, dx: f64, dy: f64) -> ControlResult<()>;
    /// 结束拖拽(拦截模式下派发 CDP `drop`)并抬起左键
    fn release(&mut self, p: Point) -> ControlResult<()>;
    /// 取选择器的可视中心
    fn center(&mut self, selector: &str) -> ControlResult<Point>;
    /// 调整容器滚动位置
    fn scroll_container(&mut self, selector: &str, dx: f64, dy: f64) -> ControlResult<()>;
    /// 等待
    fn wait(&mut self, ms: u64);
    /// finally 清理:关闭拦截、取消订阅、确保按键释放(幂等、不抛错)
    fn cleanup(&mut self);
}

/// RAII:无论正常返回还是 panic,都执行 `host.cleanup()`
struct CleanupGuard<'a, H: DragHost> {
    host: &'a mut H,
}

impl<'a, H: DragHost> CleanupGuard<'a, H> {
    /// 取回宿主引用(生命周期受 guard 约束)
    fn host(&mut self) -> &mut H {
        self.host
    }
}

impl<H: DragHost> Drop for CleanupGuard<'_, H> {
    fn drop(&mut self) {
        self.host.cleanup();
    }
}

/// 执行器状态
struct ExecState {
    /// 当前鼠标位置
    pos: Point,
    /// 左键是否按下
    pressed: bool,
    /// 最近一次 move 的目标(滚动后需按新布局重算)
    last_dest: Option<DragTarget>,
}

/// 执行批次动作;返回动作数。中途失败也会在 finally 中释放已按下的左键。
pub(crate) fn run_actions<H: DragHost>(
    host: &mut H,
    actions: &[Action],
    viewport: Viewport,
) -> ControlResult<usize> {
    let mut state = ExecState {
        pos: Point { x: 0.0, y: 0.0 },
        pressed: false,
        last_dest: None,
    };
    let mut guard = CleanupGuard { host };
    let host = guard.host();
    let mut error = run_actions_inner(host, actions, viewport, &mut state).err();
    // 仅"正常结束且未显式 release"才补 drop;错误路径只 cleanup(释放按键/取消),避免错位 drop
    if error.is_none() && state.pressed {
        if let Err(e) = host.release(state.pos) {
            error = Some(format!("drag: auto-release failed: {e}"));
        }
    }
    match error {
        Some(e) => Err(e),
        None => Ok(actions.len()),
    }
}

/// 顺序执行动作(不含 finally)
fn run_actions_inner<H: DragHost>(
    host: &mut H,
    actions: &[Action],
    viewport: Viewport,
    state: &mut ExecState,
) -> ControlResult<()> {
    for (i, action) in actions.iter().enumerate() {
        let label = format!("action #{} {}", i + 1, action_name(action));
        match action {
            Action::Wait { ms } => host.wait(*ms),
            Action::Press { target } => {
                let p = resolve_target(host, target, viewport, &label, true)?;
                // 先移动再按下(与真实鼠标一致;批次开始前没有跨命令光标状态)
                host.move_to(p)?;
                state.pos = p;
                // 先置按下:即使 press 超时(事件可能已发出)也由 finally 补 release
                state.pressed = true;
                host.press(p)?;
            }
            Action::Move {
                target,
                steps,
                delay_ms,
            } => {
                let dest = resolve_target(host, target, viewport, &label, false)?;
                state.last_dest = Some(target.clone());
                move_interpolated(host, state, dest, *steps, *delay_ms)?;
            }
            Action::Scroll { selector, dx, dy } => {
                match selector {
                    // 拦截模式:只调整容器滚动,不再额外注入 wheel(避免双倍滚动)
                    Some(sel) => {
                        resolve_target_selector(host, sel, &label)?;
                        host.scroll_container(sel, *dx, *dy)?;
                    }
                    // 无容器时仍用 wheel 滚动指针下的内容
                    None => host.wheel(state.pos, *dx, *dy)?,
                }
                host.wait(SCROLL_SETTLE_MS);
                // 滚动会改变布局:按最近一次 move 目标重算坐标(无则保持当前点),再 dragOver
                let p = match &state.last_dest {
                    Some(t) => resolve_target(host, t, viewport, &label, false)?,
                    None => state.pos,
                };
                host.drag_to(p)?;
                state.pos = p;
            }
            Action::Release { target } => {
                let p = match target {
                    Some(t) => resolve_target(host, t, viewport, &label, false)?,
                    None => state.pos,
                };
                // 先对最终目标派发 dragOver,确保页面先收到 dragover 再触发 drop
                host.drag_to(p)?;
                host.release(p)?;
                state.pos = p;
                state.pressed = false;
            }
        }
    }
    Ok(())
}

/// 分步插值移动(含可选步间等待)
fn move_interpolated<H: DragHost>(
    host: &mut H,
    state: &mut ExecState,
    dest: Point,
    steps: u32,
    delay_ms: u64,
) -> ControlResult<()> {
    let from = state.pos;
    for i in 1..=steps {
        let t = f64::from(i) / f64::from(steps);
        let p = Point {
            x: from.x + (dest.x - from.x) * t,
            y: from.y + (dest.y - from.y) * t,
        };
        host.drag_to(p)?;
        state.pos = p;
        if delay_ms > 0 && i < steps {
            host.wait(delay_ms);
        }
    }
    Ok(())
}

/// 解析目标并按需做视口校验(坐标 press 必须落在视口内)
fn resolve_target<H: DragHost>(
    host: &mut H,
    target: &DragTarget,
    viewport: Viewport,
    label: &str,
    must_be_visible: bool,
) -> ControlResult<Point> {
    match target {
        DragTarget::Selector(sel) => resolve_target_selector(host, sel, label),
        DragTarget::Point(x, y) => {
            let p = Point { x: *x, y: *y };
            if must_be_visible && !point_in_viewport(p, viewport) {
                return Err(format!(
                    "drag: {label} point out of viewport ({x},{y}; viewport {}x{})",
                    viewport.width, viewport.height
                ));
            }
            Ok(p)
        }
    }
}

/// 解析选择器可视中心,并把错误统一成 `drag: <label> ...`
fn resolve_target_selector<H: DragHost>(
    host: &mut H,
    selector: &str,
    label: &str,
) -> ControlResult<Point> {
    host.center(selector)
        .map_err(|e| format!("drag: {label} {e}"))
}

/// Windows CDP 宿主
#[cfg(windows)]
struct WindowsHost<'a> {
    /// 应用句柄
    app: &'a AppHandle,
    /// 拖拽开始时固定的 page webview label(切 tab 后仍定向到原页)
    page_label: String,
    /// 每一步 CDP/eval 的预算
    timeout_ms: Option<u64>,
    /// 是否**尝试过**开启拦截(即使 true 调用超时/失败,cleanup 仍要尝试关闭)
    intercept_attempted: bool,
    /// 左键是否仍处于按下状态
    mouse_down: bool,
    /// 是否已拿到 `dragIntercepted` 数据(拦截拖拽已开始)
    dragging: bool,
    /// 是否已派发过 `dragEnter`
    entered: bool,
    /// `Input.dragIntercepted.data`
    drag_data: Option<Value>,
    /// 最近一次鼠标/拖拽坐标(清理时用于释放)
    last: Point,
    /// `Input.dragIntercepted` 订阅 id
    drag_sub: Option<u64>,
}

#[cfg(windows)]
impl<'a> WindowsHost<'a> {
    /// 新建宿主:固定当前激活页,后续所有 CDP/eval/鼠标事件都定向到该 label
    fn new(app: &'a AppHandle, timeout_ms: Option<u64>) -> ControlResult<Self> {
        let page = crate::ui::active_page_webview(app).ok_or("page webview not ready")?;
        Ok(Self {
            app,
            page_label: page.label().to_string(),
            timeout_ms,
            intercept_attempted: false,
            mouse_down: false,
            dragging: false,
            entered: false,
            drag_data: None,
            last: Point { x: 0.0, y: 0.0 },
            drag_sub: None,
        })
    }

    /// 发送鼠标事件(固定目标页)
    fn mouse_event(
        &self,
        typ: &str,
        p: Point,
        button: &str,
        buttons: i64,
        click_count: i64,
    ) -> ControlResult<()> {
        super::mouse::mouse_event_label_with(
            self.app,
            &self.page_label,
            typ,
            p.x,
            p.y,
            button,
            buttons,
            click_count,
            self.timeout_ms,
        )
    }

    /// 开关拖拽拦截(固定目标页)
    fn set_intercept(&self, enabled: bool) -> ControlResult<()> {
        let params = serde_json::json!({ "enabled": enabled }).to_string();
        super::cdp::call_json_label_opt(
            self.app,
            &self.page_label,
            "Input.setInterceptDrags",
            &params,
            self.timeout_ms,
        )?;
        Ok(())
    }

    /// 派发 `Input.dispatchDragEvent`(固定目标页)
    fn dispatch_drag(&self, typ: &str, p: Point, data: &Value) -> ControlResult<()> {
        let params = serde_json::json!({
            "type": typ,
            "x": p.x,
            "y": p.y,
            "data": data,
        })
        .to_string();
        super::cdp::call_json_label_opt(
            self.app,
            &self.page_label,
            "Input.dispatchDragEvent",
            &params,
            self.timeout_ms,
        )?;
        Ok(())
    }

}

/// 等待 `Input.dragIntercepted` 的 dragData;`budget_ms` 为上限(沿用单步预算)
#[cfg(windows)]
fn wait_drag_data(
    rx: std::sync::mpsc::Receiver<Value>,
    budget_ms: u64,
) -> ControlResult<Value> {
    match rx.recv_timeout(Duration::from_millis(budget_ms)) {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err("drag: not started (dragIntercepted missing data)".into()),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err("drag: not started (no dragIntercepted event)".into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("drag: not started (dragIntercepted channel closed)".into())
        }
    }
}

#[cfg(windows)]
impl DragHost for WindowsHost<'_> {
    fn move_to(&mut self, p: Point) -> ControlResult<()> {
        self.mouse_event("mouseMoved", p, "none", 0, 0)?;
        self.last = p;
        Ok(())
    }

    fn press(&mut self, p: Point) -> ControlResult<()> {
        if self.mouse_down {
            return Err("drag: mouse already pressed".into());
        }
        // 先订阅事件,再开启拦截,避免 dragIntercepted 早于订阅到达;
        // tab 由固定 label 反查,切 tab 不会让订阅跟到新页
        let tab = crate::ui::tab_id_for_label(self.app, &self.page_label).unwrap_or(0);
        let (id, rx) = crate::capture::drag_intercepted_subscribe(tab);
        self.drag_sub = Some(id);
        // 先标记"已尝试":调用超时但浏览器已开启时,cleanup 仍会关闭拦截
        self.intercept_attempted = true;
        self.set_intercept(true)?;
        self.last = p;
        // 先置按下:即使 press 调用失败/超时,cleanup 也会补 release
        self.mouse_down = true;
        self.mouse_event("mousePressed", p, "left", 1, 1)?;
        // 1~2 次小位移触发浏览器拖拽拦截
        for dx in [2.0_f64, 4.0] {
            self.mouse_event(
                "mouseMoved",
                Point {
                    x: p.x + dx,
                    y: p.y,
                },
                "left",
                1,
                0,
            )?;
        }
        let budget = self.timeout_ms.unwrap_or(super::cdp::CDP_TIMEOUT_MS);
        let data = wait_drag_data(rx, budget)?;
        self.drag_data = Some(data);
        self.dragging = true;
        Ok(())
    }

    fn drag_to(&mut self, p: Point) -> ControlResult<()> {
        if !self.dragging {
            return Err("drag: not started (no active drag)".into());
        }
        let data = self
            .drag_data
            .clone()
            .ok_or("drag: not started (missing drag data)")?;
        let typ = if self.entered { "dragOver" } else { "dragEnter" };
        self.dispatch_drag(typ, p, &data)?;
        self.entered = true;
        self.last = p;
        Ok(())
    }

    fn wheel(&mut self, p: Point, dx: f64, dy: f64) -> ControlResult<()> {
        super::mouse::mouse_wheel_label_with(
            self.app,
            &self.page_label,
            p.x,
            p.y,
            dx,
            dy,
            self.timeout_ms,
        )
    }

    fn release(&mut self, p: Point) -> ControlResult<()> {
        self.last = p;
        let mut result = Ok(());
        if self.dragging {
            match self.drag_data.clone() {
                Some(data) => {
                    if let Err(e) = self.dispatch_drag("drop", p, &data) {
                        result = Err(e);
                    } else {
                        self.dragging = false;
                        self.entered = false;
                    }
                }
                None => result = Err("drag: not started (missing drag data)".into()),
            }
        }
        // 即便 drop 失败也尝试抬键;失败时留待 cleanup 再兜底
        if self.mouse_down {
            match self.mouse_event("mouseReleased", p, "left", 0, 1) {
                Ok(()) => self.mouse_down = false,
                Err(e) if result.is_ok() => result = Err(e),
                Err(_) => {}
            }
        }
        result
    }

    fn center(&mut self, selector: &str) -> ControlResult<Point> {
        let v = eval_label_value(
            self.app,
            &self.page_label,
            &center_script(selector),
            self.timeout_ms,
        )?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(format!("{err} ({selector})"));
        }
        let x = v.get("x").and_then(Value::as_f64);
        let y = v.get("y").and_then(Value::as_f64);
        match (x, y) {
            (Some(x), Some(y)) => Ok(Point { x, y }),
            _ => Err(format!("failed to read center ({selector})")),
        }
    }

    fn scroll_container(&mut self, selector: &str, dx: f64, dy: f64) -> ControlResult<()> {
        let v = eval_label_value(
            self.app,
            &self.page_label,
            &scroll_container_script(selector, dx, dy),
            self.timeout_ms,
        )?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(format!("{err} ({selector})"));
        }
        Ok(())
    }

    fn wait(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    fn cleanup(&mut self) {
        // 依据"已尝试"关闭拦截:开启调用即使超时,浏览器侧也可能已生效
        if self.intercept_attempted {
            if let Err(e) = self.set_intercept(false) {
                // 不掩盖原始错误,但必须记录
                eprintln!("[drag] setInterceptDrags(false) failed: {e}");
            }
            self.intercept_attempted = false;
        }
        if self.mouse_down {
            let _ = self.mouse_event("mouseReleased", self.last, "left", 0, 1);
            self.mouse_down = false;
        }
        if let Some(id) = self.drag_sub.take() {
            crate::capture::drag_intercepted_unsubscribe(id);
        }
        self.dragging = false;
        self.entered = false;
        self.drag_data = None;
    }
}
/// 读取指定 page webview 的视口尺寸
#[cfg(windows)]
fn read_viewport(
    app: &AppHandle,
    label: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Viewport> {
    let v = eval_label_value(app, label, viewport_script(), timeout_ms)?;
    let width = v.get("width").and_then(Value::as_f64).unwrap_or(0.0);
    let height = v.get("height").and_then(Value::as_f64).unwrap_or(0.0);
    if width <= 0.0 || height <= 0.0 {
        return Err("drag: viewport is empty".into());
    }
    Ok(Viewport { width, height })
}

/// 非批次拖拽核心(纯宿主逻辑,便于单测「先解析后发事件」与 finally 释放)
pub(crate) fn run_drag<H: DragHost>(
    host: &mut H,
    source: &DragTarget,
    target: &DragTarget,
    viewport: Viewport,
    steps: usize,
) -> ControlResult<usize> {
    // guard 在正常返回/panic 时都会清理:关闭拦截、取消订阅、释放按键
    let mut guard = CleanupGuard { host };
    run_drag_inner(guard.host(), source, target, viewport, steps)
}

/// 非批次拖拽内层:先解析两个点,再 move → press(启动拦截) → dragEnter/Over → CDP drop
fn run_drag_inner<H: DragHost>(
    host: &mut H,
    source: &DragTarget,
    target: &DragTarget,
    viewport: Viewport,
    steps: usize,
) -> ControlResult<usize> {
    // 两个点都解析成功后才允许发鼠标事件
    let from = resolve_target(host, source, viewport, "source", true)?;
    let to = resolve_target(host, target, viewport, "target", false)?;

    host.move_to(from)?;
    host.press(from)?;
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let p = Point {
            x: from.x + (to.x - from.x) * t,
            y: from.y + (to.y - from.y) * t,
        };
        host.drag_to(p)?;
    }
    host.release(to)?;
    Ok(steps)
}

/// 非批次拖拽:先取源/目标可视中心,再注入 move → press → N 步 move → release
///
/// 不可见/找不到时在**发出任何鼠标事件之前**返回错误。
pub fn drag_targets_with(
    app: &AppHandle,
    source: DragTarget,
    target: DragTarget,
    timeout_ms: Option<u64>,
    pointer: Pointer,
) -> ControlResult<DragDone> {
    #[cfg(windows)]
    {
        drag_targets_windows(app, &source, &target, timeout_ms, pointer)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, source, target, timeout_ms, pointer);
        Err(UNSUPPORTED.into())
    }
}

/// Windows 实现
#[cfg(windows)]
fn drag_targets_windows(
    app: &AppHandle,
    source: &DragTarget,
    target: &DragTarget,
    timeout_ms: Option<u64>,
    pointer: Pointer,
) -> ControlResult<DragDone> {
    let start = Instant::now();
    let steps = match pointer {
        Pointer::Mouse => {
            let mut host = WindowsHost::new(app, timeout_ms)?;
            let viewport = read_viewport(app, &host.page_label, timeout_ms)?;
            run_drag(&mut host, source, target, viewport, DRAG_STEPS)?
        }
        Pointer::Touch => {
            let mut host = super::touch::TouchDragHost::new(app, timeout_ms)?;
            let viewport = read_viewport(app, &host.page_label, timeout_ms)?;
            run_drag(&mut host, source, target, viewport, DRAG_STEPS)?
        }
    };
    Ok(DragDone {
        steps,
        elapsed_ms: start.elapsed().as_millis() as u64,
    })
}

/// 批次拖拽:单次调用内顺序执行;无论成败都保证释放已按下的左键
pub fn drag_actions_with(
    app: &AppHandle,
    actions: &[Action],
    timeout_ms: Option<u64>,
    pointer: Pointer,
) -> ControlResult<DragDone> {
    #[cfg(windows)]
    {
        let start = Instant::now();
        let steps = match pointer {
            Pointer::Mouse => {
                let mut host = WindowsHost::new(app, timeout_ms)?;
                let viewport = read_viewport(app, &host.page_label, timeout_ms)?;
                run_actions(&mut host, actions, viewport)?
            }
            Pointer::Touch => {
                let mut host = super::touch::TouchDragHost::new(app, timeout_ms)?;
                let viewport = read_viewport(app, &host.page_label, timeout_ms)?;
                run_actions(&mut host, actions, viewport)?
            }
        };
        Ok(DragDone {
            steps,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (app, actions, timeout_ms, pointer);
        Err(UNSUPPORTED.into())
    }
}

/// 测试宿主:记录事件;可让指定次数的 `center` 调用失败
#[cfg(test)]
#[derive(Default)]
struct FakeHost {
    events: Vec<String>,
    centers: HashMap<String, Point>,
    /// scroll_container 后更新的选择器中心(模拟滚动改变布局)
    scroll_updates: HashMap<String, Point>,
    fail_move_at: Option<usize>,
    fail_press: bool,
    panic_on_drag: bool,
    move_count: usize,
    cleaned: bool,
}

#[cfg(test)]
impl DragHost for FakeHost {
    fn move_to(&mut self, p: Point) -> ControlResult<()> {
        self.move_count += 1;
        if self.fail_move_at == Some(self.move_count) {
            return Err("synthetic move failure".into());
        }
        self.events.push(format!("move {:.1},{:.1}", p.x, p.y));
        Ok(())
    }

    fn press(&mut self, p: Point) -> ControlResult<()> {
        if self.fail_press {
            return Err("drag: not started (no dragIntercepted event)".into());
        }
        self.events.push(format!("press {:.1},{:.1}", p.x, p.y));
        Ok(())
    }

    fn drag_to(&mut self, p: Point) -> ControlResult<()> {
        self.move_count += 1;
        if self.fail_move_at == Some(self.move_count) {
            return Err("synthetic move failure".into());
        }
        if self.panic_on_drag {
            panic!("synthetic drag panic");
        }
        self.events.push(format!("drag {:.1},{:.1}", p.x, p.y));
        Ok(())
    }

    fn wheel(&mut self, p: Point, dx: f64, dy: f64) -> ControlResult<()> {
        self.events
            .push(format!("wheel {:.1},{:.1} {dx},{dy}", p.x, p.y));
        Ok(())
    }

    fn release(&mut self, p: Point) -> ControlResult<()> {
        self.events.push(format!("release {:.1},{:.1}", p.x, p.y));
        Ok(())
    }

    fn center(&mut self, selector: &str) -> ControlResult<Point> {
        self.centers
            .get(selector)
            .copied()
            .ok_or_else(|| format!("not found ({selector})"))
    }

    fn scroll_container(&mut self, selector: &str, dx: f64, dy: f64) -> ControlResult<()> {
        self.events
            .push(format!("scroll-container {selector} {dx},{dy}"));
        let updates = self.scroll_updates.clone();
        for (key, point) in updates {
            self.centers.insert(key, point);
        }
        Ok(())
    }

    fn wait(&mut self, ms: u64) {
        self.events.push(format!("wait {ms}"));
    }

    fn cleanup(&mut self) {
        self.cleaned = true;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[cfg(windows)]
    use super::wait_drag_data;
    use super::{
        Action, DragTarget, FakeHost, MAX_MOVE_STEPS, Point, Viewport, center_script, parse_actions,
        parse_point, point_in_viewport, run_actions, run_drag, scroll_container_script,
        validate_actions, viewport_script,
    };

    fn vp(w: f64, h: f64) -> Viewport {
        Viewport {
            width: w,
            height: h,
        }
    }

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn parse_point_accepts_spaces_and_rejects_bad_input() {
        assert_eq!(parse_point(" 10 , 20.5 ").unwrap(), (10.0, 20.5));
        assert_eq!(parse_point("-1,0").unwrap(), (-1.0, 0.0));
        assert!(parse_point("1").is_err());
        assert!(parse_point("1,2,3").is_err());
        assert!(parse_point("NaN,2").is_err());
        assert!(parse_point("inf,2").is_err());
        assert!(parse_point(",2").is_err());
    }

    /// 可视中心只有 JS 一处实现:校验生成脚本确实包含交集算法与错误分支
    #[test]
    fn center_script_is_the_single_visible_center_implementation() {
        let js = center_script("#card");
        assert!(js.contains("document.querySelector(\"#card\")"));
        assert!(js.contains("getBoundingClientRect"));
        assert!(js.contains("getComputedStyle"));
        assert!(js.contains("overflowX !== \"visible\""));
        assert!(js.contains("scrollWidth > p.clientWidth"));
        assert!(js.contains("window.innerWidth"));
        assert!(js.contains("borderLeftWidth"));
        assert!(js.contains("Math.max(0, r.left)"));
        assert!(js.contains("Math.min(vw, r.right)"));
        assert!(js.contains("\"not found\""));
        assert!(js.contains("\"center not visible\""));
    }

    /// 选择器按 JS 字符串字面量转义,不能注入脚本
    #[test]
    fn center_script_escapes_selector() {
        let selector = "a\"b\\c";
        assert!(center_script(selector).contains(&format!("{selector:?}")));
    }

    /// 滚动与视口脚本同样只有一处生成
    #[test]
    fn scroll_and_viewport_scripts_are_generated() {
        let scroll = scroll_container_script(".list", 0.0, 120.0);
        assert!(scroll.contains("document.querySelector(\".list\")"));
        assert!(scroll.contains("scrollLeft += 0"));
        assert!(scroll.contains("scrollTop += 120"));
        assert!(viewport_script().contains("window.innerWidth"));
        assert!(viewport_script().contains("window.innerHeight"));
    }

    #[test]
    fn press_point_requires_strict_viewport_bounds() {
        let viewport = vp(100.0, 50.0);
        assert!(point_in_viewport(p(0.0, 0.0), viewport));
        assert!(point_in_viewport(p(99.9, 49.9), viewport));
        assert!(!point_in_viewport(p(100.0, 10.0), viewport));
        assert!(!point_in_viewport(p(10.0, 50.0), viewport));
        assert!(!point_in_viewport(p(-1.0, 10.0), viewport));
    }

    #[test]
    fn actions_parse_legal_sequence_and_reject_click_type() {
        let legal = r#"[{"press":{"to":"10,10"}},{"move":{"to":"50,50","steps":2}},{"scroll":{"dy":100}},{"wait":{"ms":5}},{"release":{"to":"60,60"}}]"#;
        let actions = parse_actions(legal).unwrap();
        assert_eq!(actions.len(), 5);
        assert!(parse_actions(r##"[{"click":{"selector":"#a"}}]"##).is_err());
        assert!(parse_actions(r#"[{"type":{"text":"x"}}]"#).is_err());
        assert!(parse_actions("[]").is_err());
        assert!(parse_actions("not json").is_err());
    }

    #[test]
    fn actions_state_machine_rejects_illegal_sequences() {
        // move/release/scroll 在 press 之前
        for bad in [
            r#"[{"move":{"to":"1,1"}},{"release":{"to":"2,2"}}]"#,
            r#"[{"scroll":{"dy":10}},{"press":{"to":"1,1"}}]"#,
            r#"[{"release":{"to":"1,1"}}]"#,
        ] {
            assert!(parse_actions(bad).is_err(), "should reject {bad}");
        }
        // release 之后不能再有动作
        assert!(
            parse_actions(r#"[{"press":{"to":"1,1"}},{"release":{}},{"wait":{"ms":1}}]"#).is_err()
        );
        // 双击/双 press
        assert!(
            parse_actions(r#"[{"press":{"to":"1,1"}},{"press":{"to":"1,1"}}]"#).is_err()
        );
        // 合法:press -> move -> release;或 press 后省略 release 由 finally 补
        assert!(parse_actions(r#"[{"press":{"to":"1,1"}},{"move":{"to":"2,2"}}]"#).is_ok());
        // 直接跑一次 validate 以覆盖手动构造的序列
        let manual = vec![
            Action::Press {
                target: DragTarget::Point(1.0, 1.0),
            },
            Action::Release { target: None },
        ];
        assert!(validate_actions(&manual).is_ok());
    }

    #[test]
    fn batch_rejects_press_outside_viewport_before_any_event() {
        let actions = vec![Action::Press {
            target: DragTarget::Point(200.0, 10.0),
        }];
        let mut host = FakeHost::default();
        let err = run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap_err();
        assert!(err.contains("out of viewport"), "got: {err}");
        assert!(host.events.is_empty(), "no mouse events expected: {:?}", host.events);
    }

    #[test]
    fn batch_allows_release_outside_viewport() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Move {
                target: DragTarget::Point(200.0, 200.0),
                steps: 1,
                delay_ms: 0,
            },
            Action::Release {
                target: Some(DragTarget::Point(300.0, 300.0)),
            },
        ];
        let mut host = FakeHost::default();
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "drag 200.0,200.0",
                "drag 300.0,300.0",
                "release 300.0,300.0"
            ]
        );
    }

    #[test]
    fn batch_error_does_not_drop_but_cleans_up() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Move {
                target: DragTarget::Point(80.0, 80.0),
                steps: 1,
                delay_ms: 0,
            },
            Action::Release { target: None },
        ];
        let mut host = FakeHost {
            fail_move_at: Some(2),
            ..Default::default()
        };
        let err = run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap_err();
        assert!(err.contains("synthetic move failure"), "got: {err}");
        assert!(host.cleaned, "cleanup must run on failure");
        // 错误路径只清理,不产生 drop(release)事件
        assert_eq!(host.events, vec!["move 10.0,10.0", "press 10.0,10.0"]);
    }

    #[test]
    fn batch_auto_release_when_release_action_omitted() {
        let actions = vec![Action::Press {
            target: DragTarget::Point(10.0, 10.0),
        }];
        let mut host = FakeHost::default();
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec!["move 10.0,10.0", "press 10.0,10.0", "release 10.0,10.0"]
        );
    }

    #[test]
    fn batch_selector_failure_does_not_drop() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Move {
                target: DragTarget::Selector("#missing".into()),
                steps: 1,
                delay_ms: 0,
            },
        ];
        let mut host = FakeHost::default();
        let err = run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap_err();
        assert!(err.contains("not found"), "got: {err}");
        assert!(host.cleaned);
        assert_eq!(host.events, vec!["move 10.0,10.0", "press 10.0,10.0"]);
    }

    /// 批次 scroll:容器可视中心校验 → scrollLeft/Top → wheel → 等待;结束后仍释放
    #[test]
    fn batch_scroll_with_selector_scrolls_container_then_dragover() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Scroll {
                selector: Some(".list".into()),
                dx: 0.0,
                dy: 120.0,
            },
        ];
        let mut host = FakeHost {
            centers: HashMap::from([(".list".to_string(), p(50.0, 50.0))]),
            ..Default::default()
        };
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "scroll-container .list 0,120",
                "wait 40",
                "drag 10.0,10.0",
                "release 10.0,10.0",
            ]
        );
    }

    /// 批次 scroll 无 selector:仍用 wheel 滚动指针下内容,随后 dragOver
    #[test]
    fn batch_scroll_without_selector_uses_wheel() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Scroll {
                selector: None,
                dx: 0.0,
                dy: 120.0,
            },
        ];
        let mut host = FakeHost::default();
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "wheel 10.0,10.0 0,120",
                "wait 40",
                "drag 10.0,10.0",
                "release 10.0,10.0",
            ]
        );
    }

    /// 滚动后必须按新布局重算 selector 目标,再 dragOver;release 也要重算
    #[test]
    fn batch_scroll_recomputes_target_before_release() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Move {
                target: DragTarget::Selector("#target".into()),
                steps: 1,
                delay_ms: 0,
            },
            Action::Scroll {
                selector: Some("#body".into()),
                dx: 0.0,
                dy: 300.0,
            },
            Action::Release {
                target: Some(DragTarget::Selector("#target".into())),
            },
        ];
        let mut host = FakeHost {
            centers: HashMap::from([
                ("#target".into(), p(20.0, 20.0)),
                ("#body".into(), p(50.0, 50.0)),
            ]),
            scroll_updates: HashMap::from([("#target".into(), p(70.0, 70.0))]),
            ..Default::default()
        };
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "drag 20.0,20.0",
                "scroll-container #body 0,300",
                "wait 40",
                "drag 70.0,70.0",
                "drag 70.0,70.0",
                "release 70.0,70.0",
            ]
        );
    }

    /// 实测失败场景:press -> scroll -> wait -> release(selector) 无 move
    #[test]
    fn batch_scroll_then_release_selector_uses_post_scroll_center() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Scroll {
                selector: Some("#body".into()),
                dx: 0.0,
                dy: 300.0,
            },
            Action::Wait { ms: 300 },
            Action::Release {
                target: Some(DragTarget::Selector("#target".into())),
            },
        ];
        let mut host = FakeHost {
            centers: HashMap::from([
                ("#target".into(), p(20.0, 20.0)),
                ("#body".into(), p(50.0, 50.0)),
            ]),
            scroll_updates: HashMap::from([("#target".into(), p(80.0, 80.0))]),
            ..Default::default()
        };
        run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap();
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "scroll-container #body 0,300",
                "wait 40",
                "drag 10.0,10.0",
                "wait 300",
                "drag 80.0,80.0",
                "release 80.0,80.0",
            ]
        );
    }

    /// 仅验证脚本生成:非法 selector 被 try/catch 包裹(真实浏览器行为由 E2E 覆盖)
    #[test]
    fn center_and_scroll_scripts_generate_invalid_selector_guards() {
        let center = center_script(".item[data-id=1] .handle");
        assert!(center.contains("catch (e)"));
        assert!(center.contains("invalid selector"));
        assert!(center.contains("eval failed"));
        let scroll = scroll_container_script(".item[data-id=1] .handle", 0.0, 10.0);
        assert!(scroll.contains("catch (e)"));
        assert!(scroll.contains("invalid selector"));
    }

    /// dragIntercepted 等待必须有上限:合法 data 成功,超时/断链明确报错
    #[cfg(windows)]
    #[test]
    fn wait_drag_data_is_bounded_and_validated() {
        use std::sync::mpsc;

        let (tx, rx) = mpsc::channel();
        tx.send(serde_json::json!({ "items": [] })).unwrap();
        assert!(wait_drag_data(rx, 50).is_ok());

        let (_tx, rx) = mpsc::channel();
        let err = wait_drag_data(rx, 10).unwrap_err();
        assert!(err.contains("no dragIntercepted event"), "got: {err}");

        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        drop(tx);
        let err = wait_drag_data(rx, 50).unwrap_err();
        assert!(err.contains("channel closed"), "got: {err}");

        // 非对象 data 分支
        let (tx, rx) = mpsc::channel();
        tx.send(serde_json::json!("not-an-object")).unwrap();
        let err = wait_drag_data(rx, 50).unwrap_err();
        assert!(err.contains("missing data"), "got: {err}");
    }

    /// 非批次:目标不可见时在发任何鼠标事件之前返回错误
    #[test]
    fn drag_resolves_both_targets_before_any_mouse_event() {
        let source = DragTarget::Selector("#source".into());
        let target = DragTarget::Selector("#missing".into());
        let mut host = FakeHost {
            centers: HashMap::from([("#source".into(), p(10.0, 10.0))]),
            ..Default::default()
        };
        let err = run_drag(&mut host, &source, &target, vp(100.0, 100.0), 8).unwrap_err();
        assert!(err.contains("target not found"), "got: {err}");
        assert!(host.cleaned, "cleanup must run on error");
        assert!(host.events.is_empty(), "no mouse events expected: {:?}", host.events);
    }

    /// 非批次:sourcePoint 越界在发任何鼠标事件之前报错
    #[test]
    fn drag_rejects_source_point_outside_viewport_before_events() {
        let source = DragTarget::Point(200.0, 10.0);
        let target = DragTarget::Point(20.0, 20.0);
        let mut host = FakeHost::default();
        let err = run_drag(&mut host, &source, &target, vp(100.0, 100.0), 8).unwrap_err();
        assert!(err.contains("source point out of viewport"), "got: {err}");
        assert!(host.events.is_empty(), "no mouse events expected: {:?}", host.events);
    }

    /// 非批次 happy path:move → press → 插值 move → release
    #[test]
    fn drag_emits_expected_mouse_sequence() {
        let mut host = FakeHost::default();
        let steps = run_drag(
            &mut host,
            &DragTarget::Point(10.0, 10.0),
            &DragTarget::Point(90.0, 90.0),
            vp(100.0, 100.0),
            2,
        )
        .unwrap();
        assert_eq!(steps, 2);
        assert!(host.cleaned, "cleanup must run on success");
        assert_eq!(
            host.events,
            vec![
                "move 10.0,10.0",
                "press 10.0,10.0",
                "drag 50.0,50.0",
                "drag 90.0,90.0",
                "release 90.0,90.0",
            ]
        );
    }

    /// 未收到 dragIntercepted(press 报错)时也必须 finally 释放 + 清理
    #[test]
    fn drag_cleanup_when_drag_does_not_start() {
        let actions = vec![Action::Press {
            target: DragTarget::Point(10.0, 10.0),
        }];
        let mut host = FakeHost {
            fail_press: true,
            ..Default::default()
        };
        let err = run_actions(&mut host, &actions, vp(100.0, 100.0)).unwrap_err();
        assert!(err.contains("not started"), "got: {err}");
        assert!(host.cleaned, "cleanup must run when drag does not start");
        assert_eq!(host.events, vec!["move 10.0,10.0"]);
    }

    /// RAII guard:panic 路径也必须执行 cleanup
    #[test]
    fn cleanup_guard_runs_on_panic() {
        let actions = vec![
            Action::Press {
                target: DragTarget::Point(10.0, 10.0),
            },
            Action::Move {
                target: DragTarget::Point(20.0, 20.0),
                steps: 1,
                delay_ms: 0,
            },
        ];
        let mut host = FakeHost {
            panic_on_drag: true,
            ..Default::default()
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_actions(&mut host, &actions, vp(100.0, 100.0))
        }));
        assert!(result.is_err());
        assert!(host.cleaned, "RAII cleanup must run on panic");
    }

    #[test]
    fn move_steps_are_capped() {
        let too_many = format!(
            r#"[{{"press":{{"to":"1,1"}}}},{{"move":{{"to":"2,2","steps":{}}}}}]"#,
            MAX_MOVE_STEPS + 1
        );
        assert!(parse_actions(&too_many).is_err());
        let zero = r#"[{"press":{"to":"1,1"}},{"move":{"to":"2,2","steps":0}}]"#;
        assert!(parse_actions(zero).is_err());
    }
}
