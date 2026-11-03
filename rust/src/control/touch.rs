//! 触摸输入：单指/多指、长按、长按拖拽，支持 radius/force。
//!
//! Windows 走 CDP `Emulation.setTouchEmulationEnabled` + `Input.dispatchTouchEvent`；
//! 非 Windows 明确报未实现。点集状态机与解析为纯逻辑，宿主通过 `TouchSink` 注入以便单测。

use std::sync::Mutex;
use std::time::Instant;

use serde_json::{Map, Value};
use tauri::AppHandle;

use super::drag::DragHost;
use super::mouse::Point;
use super::page::{center_script, scroll_container_script};
use super::ControlResult;

/// 最大同时触摸点数
pub const MAX_TOUCH_POINTS: usize = 5;
/// move 插值步数上限
pub const MAX_TOUCH_STEPS: u32 = 1000;
/// 默认插值步数(swipe / 触摸拖拽同鼠标)
pub const DEFAULT_TOUCH_STEPS: u32 = 8;
/// 已启用触摸仿真的 page 缓存(导航/重建后失效)
#[derive(Default)]
struct EmulationCache {
    labels: Vec<String>,
}

impl EmulationCache {
    /// 是否已启用
    fn contains(&self, label: &str) -> bool {
        self.labels.iter().any(|l| l == label)
    }

    /// 记录已启用
    fn mark(&mut self, label: &str) {
        if !self.contains(label) {
            self.labels.push(label.to_string());
        }
    }

    /// 导航/页面重建后失效
    fn invalidate(&mut self, label: &str) {
        self.labels.retain(|l| l != label);
    }
}

static TOUCH_ENABLED: Mutex<EmulationCache> = Mutex::new(EmulationCache { labels: Vec::new() });
/// 非 Windows 统一错误
#[cfg(not(windows))]
const UNSUPPORTED: &str = "touch: unsupported platform (Windows only)";

/// 触摸点目标(选择器按可视中心解析)
#[derive(Debug, Clone, PartialEq)]
pub enum TouchTarget {
    /// 视口坐标
    Point(f64, f64),
    /// CSS 选择器
    Selector(String),
}

/// 输入侧点描述(radius/force 缺省 1 / 1.0)
#[derive(Debug, Clone, PartialEq)]
pub struct TouchPointSpec {
    /// 目标
    pub target: TouchTarget,
    /// 接触半径(同时映射 radiusX/radiusY)
    pub radius: f64,
    /// 压力
    pub force: f64,
}

/// 实际派发的触摸点(id 由按下顺序分配)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    /// 触摸 id
    pub id: u32,
    /// 视口 X
    pub x: f64,
    /// 视口 Y
    pub y: f64,
    /// 接触半径
    pub radius: f64,
    /// 压力
    pub force: f64,
}

/// 触摸动作
#[derive(Debug, Clone, PartialEq)]
pub enum TouchAction {
    /// 按下新的一组点(当前必须无活动点)
    Down(Vec<TouchPointSpec>),
    /// 移动;必须携带当前全部点
    Move {
        /// 目标点
        points: Vec<TouchPointSpec>,
        /// 插值步数
        steps: u32,
        /// 步间等待(swipe duration 用)
        delay_ms: u64,
    },
    /// 抬起全部点
    Up,
    /// 等待
    Wait {
        /// 毫秒
        ms: u64,
    },
}

/// 触摸命令结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchDone {
    /// 执行的动作数(快捷命令为 2/3)
    pub steps: usize,
    /// 耗时(ms)
    pub elapsed_ms: u64,
}

/// 触摸宿主抽象(便于点集状态机单测)
pub(crate) trait TouchSink {
    /// 解析选择器为视口坐标
    fn resolve(&mut self, spec: &TouchPointSpec) -> ControlResult<(f64, f64)>;
    /// 派发 touchStart/touchMove/touchEnd/touchCancel
    fn dispatch(&mut self, typ: &str, points: &[TouchPoint]) -> ControlResult<()>;
    /// 等待
    fn wait(&mut self, ms: u64);
    /// 异常取消(触摸点未抬起时)
    fn cancel(&mut self);
}

/// 解析 `touch --actions` JSON
pub fn parse_actions(json: &str) -> ControlResult<Vec<TouchAction>> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("touch: actions must be a JSON array: {e}"))?;
    let items = value
        .as_array()
        .ok_or("touch: actions must be a JSON array")?;
    if items.is_empty() {
        return Err("touch: actions must not be empty".into());
    }
    let mut actions = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        actions.push(parse_action(item).map_err(|e| format!("touch: action #{}: {e}", i + 1))?);
    }
    validate_sequence(&actions)?;
    if !matches!(actions.last(), Some(TouchAction::Up)) {
        return Err("touch: actions must end with up".into());
    }
    Ok(actions)
}

/// 解析单个动作(move 允许同级附带 steps)
fn parse_action(value: &Value) -> ControlResult<TouchAction> {
    let obj = value.as_object().ok_or("expected an object")?;
    if let Some(points) = obj.get("move") {
        ensure_allowed(obj, &["move", "steps"], "move")?;
        let points = parse_points(points, "move")?;
        let steps = match obj.get("steps") {
            Some(_) => required_u64(obj, "steps", "move")?,
            None => 1,
        };
        if steps == 0 || steps > u64::from(MAX_TOUCH_STEPS) {
            return Err(format!(
                "steps must be 1..={MAX_TOUCH_STEPS}, got: {steps}"
            ));
        }
        return Ok(TouchAction::Move {
            points,
            steps: steps as u32,
            delay_ms: 0,
        });
    }
    if obj.len() != 1 {
        let keys = obj.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(format!(
            "expected exactly one action key (down/move/up/wait), got [{keys}]"
        ));
    }
    let (name, params) = obj.iter().next().ok_or("expected an action object")?;
    match name.as_str() {
        "down" => Ok(TouchAction::Down(parse_points(params, "down")?)),
        "up" => {
            let arr = params.as_array().ok_or("up expects an array")?;
            if !arr.is_empty() {
                return Err("up expects an empty array".into());
            }
            Ok(TouchAction::Up)
        }
        "wait" => {
            let obj = params.as_object().ok_or("expected an object")?;
            ensure_allowed(obj, &["ms"], "wait")?;
            Ok(TouchAction::Wait {
                ms: required_u64(obj, "ms", "wait")?,
            })
        }
        other => Err(format!(
            "unsupported action \"{other}\" (allowed: down, move, up, wait)"
        )),
    }
}

/// 拒绝未声明字段
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

/// 读取可选数字(缺省值)
fn optional_f64(obj: &Map<String, Value>, key: &str, default: f64) -> ControlResult<f64> {
    match obj.get(key) {
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| format!("\"{key}\" must be a finite number")),
        None => Ok(default),
    }
}

/// 解析点数组(1..=5)
fn parse_points(value: &Value, name: &str) -> ControlResult<Vec<TouchPointSpec>> {
    let arr = value
        .as_array()
        .ok_or_else(|| format!("{name} expects an array of points"))?;
    if arr.is_empty() {
        return Err(format!("{name} requires at least one point"));
    }
    if arr.len() > MAX_TOUCH_POINTS {
        return Err(format!(
            "{name} supports at most {MAX_TOUCH_POINTS} points, got: {}",
            arr.len()
        ));
    }
    arr.iter()
        .map(|v| parse_point_spec(v).map_err(|e| format!("{name}: {e}")))
        .collect()
}

/// 解析一个点:`{x,y}` 或 `{selector,align?}`,可选 radius/force
fn parse_point_spec(value: &Value) -> ControlResult<TouchPointSpec> {
    let obj = value.as_object().ok_or("point must be an object")?;
    ensure_allowed(
        obj,
        &["x", "y", "selector", "align", "radius", "force"],
        "point",
    )?;
    let radius = optional_f64(obj, "radius", 1.0)?;
    let force = optional_f64(obj, "force", 1.0)?;
    if radius < 0.0 {
        return Err("radius must be >= 0".into());
    }
    if force < 0.0 {
        return Err("force must be >= 0".into());
    }
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
    if let Some(align) = obj.get("align") {
        let align = align.as_str().ok_or("align must be a string")?;
        if align != "center" {
            return Err(format!(
                "align only supports \"center\", got: \"{align}\""
            ));
        }
    }
    let x = point_number(obj, "x")?;
    let y = point_number(obj, "y")?;
    match (selector, x, y) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            Err("selector and x/y are mutually exclusive".into())
        }
        (Some(selector), None, None) => Ok(TouchPointSpec {
            target: TouchTarget::Selector(selector),
            radius,
            force,
        }),
        (None, Some(x), Some(y)) => Ok(TouchPointSpec {
            target: TouchTarget::Point(x, y),
            radius,
            force,
        }),
        (None, None, None) => Err("point requires selector or x/y".into()),
        _ => Err("point requires both x and y".into()),
    }
}

/// 可选点坐标(缺省 None;出现则必须是有限数)
fn point_number(obj: &Map<String, Value>, key: &str) -> ControlResult<Option<f64>> {
    match obj.get(key) {
        Some(v) => v
            .as_f64()
            .filter(|n| n.is_finite())
            .map(Some)
            .ok_or_else(|| format!("\"{key}\" must be a finite number")),
        None => Ok(None),
    }
}

/// 序列校验:down 前不能 move/up;move 点数必须等于当前按下数;上限 5
fn validate_sequence(actions: &[TouchAction]) -> ControlResult<()> {
    let mut active: Option<usize> = None;
    for (i, action) in actions.iter().enumerate() {
        let n = i + 1;
        match action {
            TouchAction::Down(points) => {
                if active.is_some() {
                    return Err(format!("touch: action #{n} down while points are active"));
                }
                if points.len() > MAX_TOUCH_POINTS {
                    return Err(format!(
                        "touch: action #{n} exceeds {MAX_TOUCH_POINTS} points"
                    ));
                }
                active = Some(points.len());
            }
            TouchAction::Move { points, .. } => {
                let Some(count) = active else {
                    return Err(format!("touch: action #{n} move before down"));
                };
                if points.len() != count {
                    return Err(format!(
                        "touch: action #{n} move must include all {count} active point(s), got {}",
                        points.len()
                    ));
                }
            }
            TouchAction::Up => {
                if active.is_none() {
                    return Err(format!("touch: action #{n} up before down"));
                }
                active = None;
            }
            TouchAction::Wait { .. } => {}
        }
    }
    Ok(())
}

/// 执行点集动作;异常时尝试 touchCancel
pub(crate) fn run_actions<S: TouchSink>(
    sink: &mut S,
    actions: &[TouchAction],
) -> ControlResult<usize> {
    let mut active: Vec<TouchPoint> = Vec::new();
    let result = run_actions_inner(sink, actions, &mut active);
    if result.is_err() && !active.is_empty() {
        sink.cancel();
        active.clear();
    }
    result.map(|()| actions.len())
}

/// 顺序执行点集动作
fn run_actions_inner<S: TouchSink>(
    sink: &mut S,
    actions: &[TouchAction],
    active: &mut Vec<TouchPoint>,
) -> ControlResult<()> {
    for (i, action) in actions.iter().enumerate() {
        let label = format!("action #{}", i + 1);
        match action {
            TouchAction::Down(points) => {
                if !active.is_empty() {
                    return Err(format!("touch: {label} down while points are active"));
                }
                let points = resolve_points(sink, points, &label)?;
                // 先标记"已尝试按下":dispatch 超时但实际已送达时,异常清理仍会 touchCancel
                *active = points;
                sink.dispatch("touchStart", active.as_slice())?;
            }
            TouchAction::Move {
                points,
                steps,
                delay_ms,
            } => {
                if active.is_empty() {
                    return Err(format!("touch: {label} move before down"));
                }
                if points.len() != active.len() {
                    return Err(format!(
                        "touch: {label} move must include all {} active point(s), got {}",
                        active.len(),
                        points.len()
                    ));
                }
                let targets = resolve_points(sink, points, &label)?;
                move_points(sink, active, &targets, *steps, *delay_ms)?;
            }
            TouchAction::Up => {
                if active.is_empty() {
                    return Err(format!("touch: {label} up before down"));
                }
                sink.dispatch("touchEnd", &[])?;
                active.clear();
            }
            TouchAction::Wait { ms } => sink.wait(*ms),
        }
    }
    Ok(())
}

/// 解析一组点(按顺序分配 id)
fn resolve_points<S: TouchSink>(
    sink: &mut S,
    specs: &[TouchPointSpec],
    label: &str,
) -> ControlResult<Vec<TouchPoint>> {
    let mut out = Vec::with_capacity(specs.len());
    for (id, spec) in specs.iter().enumerate() {
        let (x, y) = sink
            .resolve(spec)
            .map_err(|e| format!("touch: {label} {e}"))?;
        out.push(TouchPoint {
            id: id as u32,
            x,
            y,
            radius: spec.radius,
            force: spec.force,
        });
    }
    Ok(out)
}

/// 多指插值移动(保持 id)
fn move_points<S: TouchSink>(
    sink: &mut S,
    active: &mut Vec<TouchPoint>,
    targets: &[TouchPoint],
    steps: u32,
    delay_ms: u64,
) -> ControlResult<()> {
    let from = active.clone();
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let mut points = Vec::with_capacity(from.len());
        for (i, start) in from.iter().enumerate() {
            let target = &targets[i];
            points.push(TouchPoint {
                id: start.id,
                x: start.x + (target.x - start.x) * t,
                y: start.y + (target.y - start.y) * t,
                radius: target.radius,
                force: target.force,
            });
        }
        sink.dispatch("touchMove", &points)?;
        *active = points;
        if delay_ms > 0 {
            sink.wait(delay_ms);
        }
    }
    Ok(())
}

/// 开关触摸仿真的 CDP 参数
fn emulation_params() -> String {
    serde_json::json!({
        "enabled": true,
        "maxTouchPoints": MAX_TOUCH_POINTS,
    })
    .to_string()
}

/// 确保该 page 已启用触摸仿真(同一页只调用一次)
#[cfg(windows)]
fn ensure_emulation(app: &AppHandle, label: &str, timeout_ms: Option<u64>) -> ControlResult<()> {
    let mut enabled = TOUCH_ENABLED.lock().unwrap_or_else(|e| e.into_inner());
    if enabled.contains(label) {
        return Ok(());
    }
    super::cdp::call_json_label_opt(
        app,
        label,
        "Emulation.setTouchEmulationEnabled",
        &emulation_params(),
        timeout_ms,
    )?;
    enabled.mark(label);
    Ok(())
}

/// 导航/页面重建后清除该页仿真缓存(下次触摸重新启用)
#[cfg(windows)]
pub(crate) fn invalidate_emulation(label: &str) {
    let mut enabled = TOUCH_ENABLED.lock().unwrap_or_else(|e| e.into_inner());
    enabled.invalidate(label);
}

/// 派发一次 `Input.dispatchTouchEvent`
#[cfg(windows)]
fn dispatch_touch(
    app: &AppHandle,
    label: &str,
    typ: &str,
    points: &[TouchPoint],
    timeout_ms: Option<u64>,
) -> ControlResult<()> {
    ensure_emulation(app, label, timeout_ms)?;
    let pts: Vec<Value> = points
        .iter()
        .map(|p| {
            serde_json::json!({
                "x": p.x,
                "y": p.y,
                "radiusX": p.radius,
                "radiusY": p.radius,
                "force": p.force,
                "id": p.id,
            })
        })
        .collect();
    let params = serde_json::json!({
        "type": typ,
        "touchPoints": pts,
    })
    .to_string();
    super::cdp::call_json_label_opt(
        app,
        label,
        "Input.dispatchTouchEvent",
        &params,
        timeout_ms,
    )?;
    Ok(())
}

/// 当前激活页 label
#[cfg(windows)]
fn active_label(app: &AppHandle) -> ControlResult<String> {
    let page = crate::ui::active_page_webview(app).ok_or("page webview not ready")?;
    Ok(page.label().to_string())
}

/// 解析点模板的 radius/force 缺省
fn point_spec(target: TouchTarget) -> TouchPointSpec {
    TouchPointSpec {
        target,
        radius: 1.0,
        force: 1.0,
    }
}

/// 单指 tap(touchStart → touchEnd)
pub fn tap_touch_with(
    app: &AppHandle,
    target: TouchTarget,
    timeout_ms: Option<u64>,
) -> ControlResult<TouchDone> {
    #[cfg(windows)]
    {
        let start = Instant::now();
        let label = active_label(app)?;
        let mut sink = WindowsTouchSink {
            app,
            label,
            timeout_ms,
        };
        let actions = vec![
            TouchAction::Down(vec![point_spec(target)]),
            TouchAction::Up,
        ];
        let steps = run_actions(&mut sink, &actions)?;
        Ok(TouchDone {
            steps,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (app, target, timeout_ms);
        Err(UNSUPPORTED.into())
    }
}

/// 从/到坐标(swipe)
pub fn swipe_with(
    app: &AppHandle,
    from: (f64, f64),
    to: (f64, f64),
    steps: u32,
    duration_ms: Option<u64>,
    timeout_ms: Option<u64>,
) -> ControlResult<TouchDone> {
    #[cfg(windows)]
    {
        if steps == 0 || steps > MAX_TOUCH_STEPS {
            return Err(format!("swipe: steps must be 1..={MAX_TOUCH_STEPS}, got: {steps}"));
        }
        let start = Instant::now();
        let label = active_label(app)?;
        let mut sink = WindowsTouchSink {
            app,
            label,
            timeout_ms,
        };
        let delay = duration_ms.unwrap_or(0) / u64::from(steps);
        let actions = vec![
            TouchAction::Down(vec![point_spec(TouchTarget::Point(from.0, from.1))]),
            TouchAction::Move {
                points: vec![point_spec(TouchTarget::Point(to.0, to.1))],
                steps,
                delay_ms: delay,
            },
            TouchAction::Up,
        ];
        let steps = run_actions(&mut sink, &actions)?;
        Ok(TouchDone {
            steps,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (app, from, to, steps, duration_ms, timeout_ms);
        Err(UNSUPPORTED.into())
    }
}

/// 多点动作集
pub fn touch_actions_with(
    app: &AppHandle,
    actions: &[TouchAction],
    timeout_ms: Option<u64>,
) -> ControlResult<TouchDone> {
    #[cfg(windows)]
    {
        let start = Instant::now();
        let label = active_label(app)?;
        let mut sink = WindowsTouchSink {
            app,
            label,
            timeout_ms,
        };
        let steps = run_actions(&mut sink, actions)?;
        Ok(TouchDone {
            steps,
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }
    #[cfg(not(windows))]
    {
        let _ = (app, actions, timeout_ms);
        Err(UNSUPPORTED.into())
    }
}

/// Windows 触摸宿主
#[cfg(windows)]
struct WindowsTouchSink<'a> {
    /// 应用句柄
    app: &'a AppHandle,
    /// 固定 page label
    label: String,
    /// 每步预算
    timeout_ms: Option<u64>,
}

#[cfg(windows)]
impl TouchSink for WindowsTouchSink<'_> {
    fn resolve(&mut self, spec: &TouchPointSpec) -> ControlResult<(f64, f64)> {
        match &spec.target {
            TouchTarget::Point(x, y) => Ok((*x, *y)),
            TouchTarget::Selector(sel) => {
                let v = super::page::eval_label_value(
                    self.app,
                    &self.label,
                    &center_script(sel),
                    self.timeout_ms,
                )?;
                if let Some(err) = v.get("error").and_then(Value::as_str) {
                    return Err(format!("{err} ({sel})"));
                }
                let x = v.get("x").and_then(Value::as_f64);
                let y = v.get("y").and_then(Value::as_f64);
                match (x, y) {
                    (Some(x), Some(y)) => Ok((x, y)),
                    _ => Err(format!("failed to read center ({sel})")),
                }
            }
        }
    }

    fn dispatch(&mut self, typ: &str, points: &[TouchPoint]) -> ControlResult<()> {
        dispatch_touch(self.app, &self.label, typ, points, self.timeout_ms)
    }

    fn wait(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn cancel(&mut self) {
        if let Err(e) = dispatch_touch(self.app, &self.label, "touchCancel", &[], self.timeout_ms) {
            // 不覆盖原始错误,但记录取消失败
            eprintln!("[touch] touchCancel failed: {e}");
        }
    }
}

/// 触摸版 DragHost:复用 drag 的批次/坐标/滚动语义
#[cfg(windows)]
pub(crate) struct TouchDragHost<'a> {
    /// 应用句柄
    app: &'a AppHandle,
    /// 固定 page label
    pub(crate) page_label: String,
    /// 每步预算
    timeout_ms: Option<u64>,
    /// 当前单指(仅 id=0)
    active: Option<TouchPoint>,
    /// 最近坐标
    last: Point,
}

#[cfg(windows)]
impl<'a> TouchDragHost<'a> {
    /// 新建并固定当前页
    pub(crate) fn new(app: &'a AppHandle, timeout_ms: Option<u64>) -> ControlResult<Self> {
        Ok(Self {
            app,
            page_label: active_label(app)?,
            timeout_ms,
            active: None,
            last: Point { x: 0.0, y: 0.0 },
        })
    }
}

#[cfg(windows)]
impl DragHost for TouchDragHost<'_> {
    fn move_to(&mut self, p: Point) -> ControlResult<()> {
        // 触摸没有 hover;只记录坐标,press 时从该点 touchStart
        self.last = p;
        Ok(())
    }

    fn press(&mut self, p: Point) -> ControlResult<()> {
        if self.active.is_some() {
            return Err("touch: points already active".into());
        }
        let point = TouchPoint {
            id: 0,
            x: p.x,
            y: p.y,
            radius: 1.0,
            force: 1.0,
        };
        // 先标记已尝试按下:即使 dispatch 超时/失败,cleanup 也会 touchCancel
        self.active = Some(point);
        dispatch_touch(self.app, &self.page_label, "touchStart", &[point], self.timeout_ms)?;
        self.last = p;
        Ok(())
    }

    fn drag_to(&mut self, p: Point) -> ControlResult<()> {
        let mut point = self
            .active
            .ok_or("touch: move before touchStart")?;
        point.x = p.x;
        point.y = p.y;
        dispatch_touch(self.app, &self.page_label, "touchMove", &[point], self.timeout_ms)?;
        self.active = Some(point);
        self.last = p;
        Ok(())
    }

    fn wheel(&mut self, _p: Point, dx: f64, dy: f64) -> ControlResult<()> {
        // 触摸模式无 wheel:无容器选择器时滚动页面
        let js = format!(
            "(function(){{ window.scrollBy({{left: {dx}, top: {dy}}}); return {{ok:true}}; }})()"
        );
        let v = super::page::eval_label_value(
            self.app,
            &self.page_label,
            &js,
            self.timeout_ms,
        )?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(format!("touch: scroll failed: {err}"));
        }
        Ok(())
    }

    fn release(&mut self, p: Point) -> ControlResult<()> {
        if self.active.is_some() {
            dispatch_touch(self.app, &self.page_label, "touchEnd", &[], self.timeout_ms)?;
            self.active = None;
        }
        self.last = p;
        Ok(())
    }

    fn center(&mut self, selector: &str) -> ControlResult<Point> {
        let v = super::page::eval_label_value(
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
        let v = super::page::eval_label_value(
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
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn cleanup(&mut self) {
        if self.active.is_some() {
            let _ = dispatch_touch(self.app, &self.page_label, "touchCancel", &[], self.timeout_ms);
            self.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EmulationCache, MAX_TOUCH_POINTS, TouchAction, TouchPoint, TouchPointSpec, TouchSink,
        TouchTarget, parse_actions, run_actions,
    };
    use crate::control::ControlResult;
    use crate::control::mouse::Point;

    #[derive(Default)]
    struct FakeSink {
        events: Vec<String>,
        centers: std::collections::HashMap<String, Point>,
        fail_dispatch: Option<String>,
        canceled: bool,
    }

    impl TouchSink for FakeSink {
        fn resolve(&mut self, spec: &TouchPointSpec) -> ControlResult<(f64, f64)> {
            match &spec.target {
                TouchTarget::Point(x, y) => Ok((*x, *y)),
                TouchTarget::Selector(s) => self
                    .centers
                    .get(s)
                    .map(|p| (p.x, p.y))
                    .ok_or_else(|| format!("not found ({s})")),
            }
        }

        fn dispatch(&mut self, typ: &str, points: &[TouchPoint]) -> ControlResult<()> {
            if self.fail_dispatch.as_deref() == Some(typ) {
                return Err("synthetic dispatch failure".into());
            }
            let pts = points
                .iter()
                .map(|p| format!("{}:{:.1},{:.1}:r{:.1}:f{:.1}", p.id, p.x, p.y, p.radius, p.force))
                .collect::<Vec<_>>()
                .join(" ");
            self.events.push(format!("{typ} [{pts}]"));
            Ok(())
        }

        fn wait(&mut self, ms: u64) {
            self.events.push(format!("wait {ms}"));
        }

        fn cancel(&mut self) {
            self.canceled = true;
            self.events.push("touchCancel".into());
        }
    }

    fn spec(x: f64, y: f64) -> TouchPointSpec {
        TouchPointSpec {
            target: TouchTarget::Point(x, y),
            radius: 1.0,
            force: 1.0,
        }
    }

    #[test]
    fn pointer_parse_and_five_points_are_supported() {
        use crate::control::drag::Pointer;
        assert_eq!(Pointer::parse("").unwrap(), Pointer::Mouse);
        assert_eq!(Pointer::parse("touch").unwrap(), Pointer::Touch);
        assert!(Pointer::parse("pen").is_err());

        let five = r#"[{"down":[{"x":1,"y":1},{"x":2,"y":2},{"x":3,"y":3},{"x":4,"y":4},{"x":5,"y":5}]},{"up":[]}]"#;
        assert!(parse_actions(five).is_ok());
    }

    #[test]
    fn radius_and_force_are_forwarded() {
        let actions = parse_actions(
            r#"[{"down":[{"x":10,"y":20,"radius":2.5,"force":0.4}]},{"up":[]}]"#,
        )
        .unwrap();
        let mut sink = FakeSink::default();
        run_actions(&mut sink, &actions).unwrap();
        assert_eq!(
            sink.events,
            vec!["touchStart [0:10.0,20.0:r2.5:f0.4]", "touchEnd []"]
        );
    }

    #[test]
    fn parse_long_press_and_multi_touch_are_valid() {
        let long = parse_actions(r#"[{"down":[{"x":10,"y":20}]},{"wait":{"ms":600}},{"up":[]}]"#)
            .unwrap();
        assert_eq!(long.len(), 3);

        let multi = parse_actions(
            r#"[{"down":[{"x":100,"y":200},{"x":300,"y":200}]},{"move":[{"x":100,"y":260},{"x":300,"y":260}],"steps":6},{"up":[]}]"#,
        )
        .unwrap();
        assert_eq!(multi.len(), 3);
    }

    #[test]
    fn batch_must_end_with_up_and_rejects_actions_after_up() {
        for bad in [
            r#"[{"down":[{"x":1,"y":1}]}]"#,
            r#"[{"down":[{"x":1,"y":1}]},{"wait":{"ms":1}}]"#,
            r#"[{"down":[{"x":1,"y":1}]},{"up":[]},{"move":[{"x":2,"y":2}]}]"#,
            r#"[{"down":[{"x":1,"y":1}]},{"up":[]},{"up":[]}]"#,
        ] {
            assert!(parse_actions(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn emulation_cache_is_idempotent_and_invalidates() {
        let mut cache = EmulationCache::default();
        cache.mark("page-1");
        cache.mark("page-1");
        assert!(cache.contains("page-1"));
        assert_eq!(cache.labels.len(), 1);
        cache.invalidate("page-1");
        assert!(!cache.contains("page-1"));
    }

    #[test]
    fn down_dispatch_failure_triggers_cancel() {
        let actions = parse_actions(r#"[{"down":[{"x":10,"y":20}]},{"up":[]}]"#).unwrap();
        let mut sink = FakeSink {
            fail_dispatch: Some("touchStart".into()),
            ..Default::default()
        };
        let err = run_actions(&mut sink, &actions).unwrap_err();
        assert!(err.contains("synthetic dispatch failure"), "got: {err}");
        assert!(sink.canceled);
        assert_eq!(sink.events, vec!["touchCancel"]);
    }

    #[test]
    fn parse_rejects_illegal_sequences_and_more_than_five_points() {
        for bad in [
            r#"[{"move":[{"x":1,"y":1}],"steps":1}]"#,
            r#"[{"up":[]}]"#,
            r#"[{"down":[{"x":1,"y":1}]},{"down":[{"x":2,"y":2}]}]"#,
            r#"[{"down":[{"x":1,"y":1}]},{"move":[{"x":2,"y":2},{"x":3,"y":3}]}]"#,
        ] {
            assert!(parse_actions(bad).is_err(), "should reject {bad}");
        }
        let too_many = r#"[{"down":[{"x":1,"y":1},{"x":2,"y":2},{"x":3,"y":3},{"x":4,"y":4},{"x":5,"y":5},{"x":6,"y":6}]}]"#;
        assert!(parse_actions(too_many).is_err());
        assert_eq!(MAX_TOUCH_POINTS, 5);
    }

    #[test]
    fn parse_point_variants_radius_force_and_align() {
        let actions = parse_actions(
            r##"[{"down":[{"selector":"#a","radius":2.5,"force":0.4},{"x":10,"y":20}]},{"up":[]}]"##,
        )
        .unwrap();
        match &actions[0] {
            TouchAction::Down(points) => {
                assert_eq!(points[0].radius, 2.5);
                assert_eq!(points[0].force, 0.4);
                assert_eq!(points[1].radius, 1.0);
                assert_eq!(points[1].force, 1.0);
            }
            _ => panic!("expected down"),
        }
        assert!(parse_actions(r##"[{"down":[{"selector":"#a","align":"top"}]},{"up":[]}]"##).is_err());
    }

    #[test]
    fn ids_stay_stable_and_move_includes_all_points() {
        let actions = parse_actions(
            r#"[{"down":[{"x":10,"y":10},{"x":20,"y":10}]},{"move":[{"x":10,"y":20},{"x":20,"y":20}],"steps":2},{"up":[]}]"#,
        )
        .unwrap();
        let mut sink = FakeSink::default();
        run_actions(&mut sink, &actions).unwrap();
        assert_eq!(
            sink.events,
            vec![
                "touchStart [0:10.0,10.0:r1.0:f1.0 1:20.0,10.0:r1.0:f1.0]",
                "touchMove [0:10.0,15.0:r1.0:f1.0 1:20.0,15.0:r1.0:f1.0]",
                "touchMove [0:10.0,20.0:r1.0:f1.0 1:20.0,20.0:r1.0:f1.0]",
                "touchEnd []",
            ]
        );
        assert!(!sink.canceled);
    }

    #[test]
    fn long_press_has_no_move() {
        let actions = parse_actions(r#"[{"down":[{"x":10,"y":10}]},{"wait":{"ms":600}},{"up":[]}]"#)
            .unwrap();
        let mut sink = FakeSink::default();
        run_actions(&mut sink, &actions).unwrap();
        assert_eq!(
            sink.events,
            vec!["touchStart [0:10.0,10.0:r1.0:f1.0]", "wait 600", "touchEnd []"]
        );
    }

    #[test]
    fn selector_resolution_uses_visible_center() {
        let actions =
            parse_actions(r##"[{"down":[{"selector":"#a"}]},{"up":[]}]"##).unwrap();
        let mut sink = FakeSink::default();
        sink.centers.insert("#a".into(), Point { x: 30.0, y: 40.0 });
        run_actions(&mut sink, &actions).unwrap();
        assert_eq!(
            sink.events,
            vec!["touchStart [0:30.0,40.0:r1.0:f1.0]", "touchEnd []"]
        );
    }

    #[test]
    fn error_path_cancels_active_points() {
        let actions = vec![
            TouchAction::Down(vec![spec(10.0, 10.0)]),
            TouchAction::Move {
                points: vec![spec(10.0, 20.0)],
                steps: 1,
                delay_ms: 0,
            },
        ];
        let mut sink = FakeSink {
            fail_dispatch: Some("touchMove".into()),
            ..Default::default()
        };
        let err = run_actions(&mut sink, &actions).unwrap_err();
        assert!(err.contains("synthetic dispatch failure"), "got: {err}");
        assert!(sink.canceled);
        assert_eq!(
            sink.events,
            vec!["touchStart [0:10.0,10.0:r1.0:f1.0]", "touchCancel"]
        );
    }
}
