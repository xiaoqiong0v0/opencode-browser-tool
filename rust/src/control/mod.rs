//! 控制协议层:通过 eval 驱动页面 Webview 完成页面操作(跨平台统一)
//! 所有操作在页面内执行一次性 JS,无残留脚本;截图平台分支
//! 注意:eval_with_callback 会把 JS 表达式返回值 JSON 序列化后传回,
//! 因此 JS 表达式直接返回对象/原始值,不要再包 JSON.stringify
pub mod accessibility;
/// Windows CDP 通用调用通道(截图/可访问性树/键盘注入共用)
#[cfg(windows)]
pub mod cdp;
/// 拖拽(仅 Windows:CDP 鼠标序列触发原生 HTML5 DnD)
pub mod drag;
/// Linux GDK 可信输入通道(gdk_event_put 注入键盘/指针事件)
#[cfg(target_os = "linux")]
pub mod gdk;
/// 文本输入与下拉选择(Windows:CDP 可信通道;非 Windows 降级到 DOM/JS 并标注)
pub mod input;
/// 键盘注入(Windows:CDP;Linux:GDK;其余平台明确报错)
pub mod keyboard;
/// 媒体权限放行(Windows/Linux:摄像头/麦克风统一放行,配合页面模拟脚本)
#[cfg(any(windows, target_os = "linux"))]
pub mod media;
/// 可信鼠标注入(Windows:CDP Input.dispatchMouseEvent;点击/悬停/iframe 点击)
pub mod mouse;
pub mod screenshot;
/// 文件上传(Windows:CDP DOM.setFileInputFiles)
pub mod upload;

use serde_json::{Value, json};
use tauri::AppHandle;

use crate::ui;

/// 页面操作错误
pub type ControlResult<T> = Result<T, String>;

/// 滚动/就绪轮询间隔(ms)
const POLL_MS: u64 = 50;
/// 判定"滚动已稳定"所需的连续相同采样次数
const SCROLL_STABLE_SAMPLES: u32 = 3;
/// 滚动稳定等待默认超时(ms),可用 `BT_SCROLL_TIMEOUT_MS` 覆盖(调试)
const DEFAULT_SCROLL_TIMEOUT_MS: u64 = 3000;
/// 导航就绪等待默认超时(ms),可用 `BT_NAV_TIMEOUT_MS` 覆盖(调试)
const DEFAULT_NAV_TIMEOUT_MS: u64 = 15000;
/// 后退/前进就绪等待超时(ms)
const HISTORY_TIMEOUT_MS: u64 = 5000;

/// 调试用超时覆盖:读环境变量(正整数),非法/缺省时用 default
pub fn timeout_from_env(var: &str, default_ms: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default_ms)
}

/// 滚动稳定等待超时(ms)
pub fn scroll_timeout_ms() -> u64 {
    timeout_from_env("BT_SCROLL_TIMEOUT_MS", DEFAULT_SCROLL_TIMEOUT_MS)
}

/// 导航就绪等待超时(ms)
pub fn nav_timeout_ms() -> u64 {
    timeout_from_env("BT_NAV_TIMEOUT_MS", DEFAULT_NAV_TIMEOUT_MS)
}

/// 前进/后退就绪等待超时(ms)
pub fn history_timeout_ms() -> u64 {
    timeout_from_env("BT_HISTORY_TIMEOUT_MS", HISTORY_TIMEOUT_MS)
}

/// 控制操作结果:附带"降级"说明
/// - `degraded = None`:走可信/原生通道(Windows CDP)
/// - `degraded = Some(reason)`:无可信通道,退回 untrusted DOM/JS 实现(须在返回里标注)
#[derive(Debug, Clone)]
pub struct Outcome<T> {
    /// 操作本体(坐标 / 读回值等)
    pub value: T,
    /// 降级原因;None 表示可信/原生
    pub degraded: Option<String>,
}

impl<T> Outcome<T> {
    /// 可信/原生执行
    pub fn trusted(value: T) -> Self {
        Self {
            value,
            degraded: None,
        }
    }

    /// 降级执行(untrusted DOM/JS 回退)
    pub fn degraded(value: T) -> Self {
        Self {
            value,
            degraded: Some(degrade_reason()),
        }
    }

    /// 降级执行并附自定义原因(如 Windows 上个别场景无适用可信路径)
    pub fn degraded_with(value: T, reason: impl Into<String>) -> Self {
        Self {
            value,
            degraded: Some(reason.into()),
        }
    }
}

/// 统一降级原因文本(键 = degradedReason,插件据此追加 `(degraded: ...)`)
pub fn degrade_reason() -> String {
    format!("untrusted JS fallback on {}", std::env::consts::OS)
}

/// 调试开关:置 `BT_FORCE_DEGRADED=1` 时即使在 Windows 也强制走降级分支
/// (用于在无 Linux 的环境下实测降级分支的代码路径与标注文本)
pub fn force_degraded() -> bool {
    std::env::var("BT_FORCE_DEGRADED")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 是否可走可信通道
/// - Windows:CDP
/// - Linux:GDK 事件注入(`gdk_event_put`,已实测可信)
/// - macOS:无 → false
/// 置 `BT_FORCE_DEGRADED=1` 时强制 false(便于实测降级分支)
pub fn trusted_available() -> bool {
    (cfg!(windows) || cfg!(target_os = "linux")) && !force_degraded()
}

/// 执行页面内 JS,返回解析后的值(对象/null/字符串等);默认超时 `ui::EVAL_TIMEOUT_MS`(15s)
pub fn eval(app: &AppHandle, js: &str) -> ControlResult<Value> {
    let raw = ui::eval_page(app, js)?;
    serde_json::from_str(&raw).map_err(|e| format!("parse eval result: {e}"))
}

/// 同 `eval`,但由调用方指定超时(ms)
pub fn eval_timeout(app: &AppHandle, js: &str, timeout_ms: u64) -> ControlResult<Value> {
    let raw = ui::eval_page_timeout(app, js, timeout_ms)?;
    serde_json::from_str(&raw).map_err(|e| format!("parse eval result: {e}"))
}

/// 内部统一入口:`timeout_ms = None` 走默认(与 1.8.x 完全一致),`Some(ms)` 用传入预算
/// 编排多步的链路每一步 eval 都经此,保证「每步原语等待用传入值」
pub(crate) fn eval_with(
    app: &AppHandle,
    js: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Value> {
    match timeout_ms {
        Some(ms) => eval_timeout(app, js, ms),
        None => eval(app, js),
    }
}

/// 收选择器的请求字段名(统一入口校验覆盖范围)
const SELECTOR_KEYS: &[&str] = &[
    "selector",
    "iframeSelector",
    "sourceSelector",
    "targetSelector",
];

/// 请求体级选择器校验:对已知选择器字段逐个执行 `validate_selector`(空值放行)
///
/// 参数:`body` HTTP 端点请求体
/// 返回:Ok 表示所有选择器字段可用;Err 为「字段名: 原因」,直接回给模型即可读
/// 用途:service 分发入口统一调用一次,覆盖 click/fill/iframe_click 等**所有**收选择器的端点
pub fn validate_selector_fields(body: &serde_json::Value) -> Result<(), String> {
    for key in SELECTOR_KEYS {
        if let Some(v) = body.get(key).and_then(|v| v.as_str()) {
            validate_selector(v).map_err(|e| format!("{key}: {e}"))?;
        }
    }
    Ok(())
}

/// 只能出现在选择器**开头**的非 CSS 引擎前缀(这些片段作为 CSS 子串也有合法用法,
/// 故只做前缀判定,不做任意位置扫描:避免误伤 `input[id=main]` 这类合法属性选择器)
const ENGINE_PREFIXES: &[(&str, &str)] = &[
    ("text=", "text="),
    ("xpath=", "xpath="),
    ("role=", "role="),
    ("id=", "id="),
    ("css=", "css="),
    (">>", ">>"),
];

/// 出现**在任意位置**即判定为非 CSS 的 Playwright 片段(引号外才算)
///
/// `>>`(链式选择器)、`text=`/`xpath=`(引擎前缀出现在中段,如 `div >> text=foo`)、
/// `:has-text(`(jQuery/Playwright 伪类)在合法 CSS 里不会作为裸子串出现;
/// 但属性值里可能合法包含(如 `[data-x="a>>b"]`),因此扫描必须**跳过引号内内容**。
const ENGINE_INFIX: &[(&str, &str)] = &[
    (">>", ">>"),
    ("text=", "text="),
    ("xpath=", "xpath="),
    (":has-text(", ":has-text("),
];

/// 引号感知扫描:返回选择器**引号外**首次出现的非 CSS 片段(及其标签)
///
/// 参数:`lower` 已全部转小写的选择器(ASCII 小写,字节下标与原串一致)
/// 返回:`Some(标签)` 表示命中禁用片段;`None` 表示引号外没有命中
///
/// 规则:单/双引号内一律跳过(支持 `\'`/`\"` 转义),引号外按字节匹配 `ENGINE_INFIX`;
/// 这样 `[data-x="a>>b"]` 合法放行,而 `div >> text=foo` / `:has-text("x")` 被拒。
fn find_unquoted_engine(lower: &str) -> Option<&'static str> {
    let bytes = lower.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) => {
                if b == b'\\' {
                    // 转义:跳过下一个字节(引号内不参与匹配)
                    i += 2;
                    continue;
                }
                if b == q {
                    quote = None;
                }
                i += 1;
            }
            None => {
                if b == b'\'' || b == b'"' {
                    quote = Some(b);
                    i += 1;
                    continue;
                }
                for (pattern, label) in ENGINE_INFIX {
                    if bytes[i..].starts_with(pattern.as_bytes()) {
                        return Some(label);
                    }
                }
                i += 1;
            }
        }
    }
    None
}

/// 单个选择器入口校验:拒绝 Playwright 风格选择器,统一在**进入点击/解析链路前**快速明确报错
///
/// 参数:`selector` 选择器(空串放行 —— 必填与否由各端点自己的校验决定)
/// 返回:Ok(()) 表示可继续;Err 为可读提示(本项目只支持 CSS 选择器)
///
/// 背景:`text=`/`xpath=`/以 `/` 开头等都不是合法 CSS,`querySelector` 会抛 SyntaxError,
/// 原先会把晦涩的解析错误(甚至看似「卡住」后的超时)甩给用户;现入口拦截、不进链路。
/// 1.10.0:`>>` 等也只查了开头,`div >> text=foo` 这类中段写法会漏到 querySelector →
/// 现补引号感知的任意位置扫描(不误伤 `[data-x="a>>b"]`)。
pub fn validate_selector(selector: &str) -> Result<(), String> {
    let s = selector.trim();
    if s.is_empty() {
        return Ok(());
    }
    let lower = s.to_ascii_lowercase();
    // Playwright/其它引擎前缀:本项目只支持 CSS,统一拒绝并给出可执行的改法
    for (prefix, label) in ENGINE_PREFIXES {
        if lower.starts_with(prefix) {
            return Err(engine_error(label, s));
        }
    }
    // 任意位置(引号外)出现的非 CSS 片段:如 `div >> text=foo`、`:has-text("x")`
    if let Some(label) = find_unquoted_engine(&lower) {
        return Err(engine_error(label, s));
    }
    // 以 `/` 开头 = XPath 路径(//div、/html/body 等)
    if s.starts_with('/') {
        return Err(format!(
            "不支持 XPath 路径选择器（以 / 开头），请改用 CSS 选择器（如 div > a.btn）: {s}"
        ));
    }
    Ok(())
}

/// 统一的"非 CSS 选择器"报错文案
fn engine_error(label: &str, selector: &str) -> String {
    format!(
        "不支持 {label} 选择器（Playwright 风格），请改用 CSS 选择器（如 div > a.btn）: {selector}"
    )
}

/// 导航到 URL
pub fn navigate(app: &AppHandle, url: &str) -> ControlResult<()> {
    ui::active_page_webview(app)
        .ok_or("page webview not ready")?
        .navigate(url.parse().map_err(|e| format!("invalid url: {e}"))?)
        .map_err(|e| format!("navigate failed: {e}"))
}

/// 执行任意 JS,返回结果(自动 JSON 序列化);默认超时 15s
///
/// 单表达式与旧版**逐字节一致**(`(function(){ return <script>; })()`,不需要 eval);
/// 多语句走 eval 包装(返回最后一条语句的完成值),失败再回退旧包装 —— 见 `eval_plan`
pub fn evaluate(app: &AppHandle, script: &str) -> ControlResult<Value> {
    run_eval_plan(app, &eval_plan(script), None)
}

/// 同 `evaluate`,但由调用方指定超时(ms)——`evaluate` 工具的 `--timeout` 直达点
///
/// 与 `evaluate` 共用同一套「构造 + 回退」逻辑,只差 timeout 参数
pub fn evaluate_timeout(app: &AppHandle, script: &str, timeout_ms: u64) -> ControlResult<Value> {
    run_eval_plan(app, &eval_plan(script), Some(timeout_ms))
}

/// 回退(或首试)失败时是否值得再试一次
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryOn {
    /// 只有「语法/解析错误」才回退:单表达式路径的成功分支与旧版完全一致
    SyntaxOnly,
    /// 除超时/通道类失败外都回退:eval 包装可能被 CSP 拦,也可能脚本自身抛错
    NonTransient,
}

impl RetryOn {
    /// 参数:`err` 首试的错误文案;返回:true 表示应执行回退脚本
    fn matches(self, err: &str) -> bool {
        match self {
            RetryOn::SyntaxOnly => is_syntax_error(err),
            RetryOn::NonTransient => !is_transient_failure(err),
        }
    }
}

/// 一次 `evaluate` 的脚本执行计划(纯逻辑,便于单测)
#[derive(Debug, PartialEq, Eq)]
struct EvalPlan {
    /// 首次尝试的脚本
    primary: String,
    /// 首试失败且满足 `retry_on` 时再试一次的脚本(`None` = 不回退)
    fallback: Option<String>,
    /// 回退触发条件
    retry_on: RetryOn,
}

/// 旧包装:脚本按**单个表达式**放在 `return` 之后(不依赖 eval,严格 CSP 站点可用)
fn return_wrap(script: &str) -> String {
    format!("(function(){{ return {script}; }})()")
}

/// 多语句包装:整段脚本作为**字符串字面量**交给 eval,返回最后一条语句的完成值
///
/// eval 需要页面的 `unsafe-eval` 权限;严格 CSP 站点会被拦,此时由 `eval_plan`
/// 回退到 `return_wrap`(即旧行为),不会比旧版更差。
fn eval_wrap(script: &str) -> String {
    // 必须用 JSON 字符串字面量:裸插会被脚本里的引号/换行/反斜杠截断
    let literal = match serde_json::to_string(script) {
        Ok(s) => s,
        // &str 序列化不会失败;真失败时退化为空脚本(等价于旧版报错),不 panic
        Err(_) => "\"\"".to_string(),
    };
    format!("(function(){{ return eval({literal}); }})()")
}

/// 构造执行计划:单表达式 → 只走 `return` 包装;多语句 → 走 eval 包装,失败回退旧包装
///
/// 为什么多语句不先试 `return` 包装:`return <多语句>` 要么直接 SyntaxError,
/// 要么「首句执行后 return、余下是死代码」——回退时首句副作用会被重放(如点击两次);
/// 且 WebView2 把脚本抛异常一律回成 `null`(ICoreWebView2::ExecuteScript 文档),
/// 首试既拿不到语法错误也拿不到结果,只能靠静态判定分流。
fn eval_plan(script: &str) -> EvalPlan {
    if is_single_expression(script) {
        EvalPlan {
            primary: return_wrap(script),
            // 语法/解析错误说明脚本压根没执行过,重试无副作用;也兜住静态判定漏判的多语句
            fallback: Some(eval_wrap(script)),
            retry_on: RetryOn::SyntaxOnly,
        }
    } else {
        EvalPlan {
            primary: eval_wrap(script),
            fallback: Some(return_wrap(script)),
            retry_on: RetryOn::NonTransient,
        }
    }
}

/// 按计划执行(默认超时与 `--timeout` 两处共用)
///
/// 参数:`timeout_ms` `None` = 默认 15s;返回:成功值。
/// 两次都失败时返回**首试**的错误文案(跨平台同形,不含绝对路径)。
fn run_eval_plan(
    app: &AppHandle,
    plan: &EvalPlan,
    timeout_ms: Option<u64>,
) -> ControlResult<Value> {
    let primary_err = match eval_with(app, &plan.primary, timeout_ms) {
        Ok(v) => return Ok(v),
        Err(e) => e,
    };
    let fallback = match plan.fallback.as_deref() {
        Some(js) if plan.retry_on.matches(&primary_err) => js,
        _ => return Err(primary_err),
    };
    eval_with(app, fallback, timeout_ms).map_err(|_| primary_err)
}

/// 以这些关键字开头的顶层片段不可能是单个表达式(一律按多语句处理)
const STATEMENT_KEYWORDS: &[&str] = &[
    "const", "let", "var", "if", "else", "for", "while", "do", "switch", "case", "default", "try",
    "catch", "finally", "throw", "return", "break", "continue", "class", "function", "import",
    "export", "debugger", "with",
];

/// 判断脚本能否按**单个表达式**包进 `return <script>`
///
/// 判据只看顶层(字符串/模板/注释/正则/括号内部不算):
/// - 顶层出现语句分隔 `;` → 多语句(末尾多余分号不算:`el.click();` 仍是单表达式);
/// - 顶层片段以语句关键字开头(`const`/`let`/`if`/`for`/...)→ 多语句。
///
/// 误判方向刻意偏向「单表达式」:漏判多语句只会退回旧行为(null 或语法错误),
/// 而把单表达式误判成多语句会破坏严格 CSP 站点。
fn is_single_expression(script: &str) -> bool {
    let bytes = script.as_bytes();
    let mut segments: Vec<&str> = Vec::new();
    let mut depth: i32 = 0;
    let mut seg_start = 0usize;
    let mut prev: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        // 字符串/模板/注释/正则整体跳过:内部的分隔符不属于顶层
        let skipped = match c {
            b'"' | b'\'' => Some(skip_quoted(bytes, i, c)),
            b'`' => Some(skip_template(bytes, i)),
            b'/' if bytes.get(i + 1) == Some(&b'/') => Some(skip_line_comment(bytes, i)),
            b'/' if bytes.get(i + 1) == Some(&b'*') => Some(skip_block_comment(bytes, i)),
            b'/' if regex_allowed(prev) => Some(skip_regex(bytes, i)),
            _ => None,
        };
        if let Some(next) = skipped {
            prev = Some(c);
            i = next;
            continue;
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            // depth <= 0:容忍多余的右括号,不让它掩盖真正的顶层分号
            b';' if depth <= 0 => {
                segments.push(&script[seg_start..i]);
                seg_start = i + 1;
            }
            _ => {}
        }
        if !c.is_ascii_whitespace() {
            prev = Some(c);
        }
        i += 1;
    }
    segments.push(&script[seg_start..]);
    let mut parts = segments.iter().map(|s| s.trim()).filter(|s| !s.is_empty());
    let first = match parts.next() {
        Some(s) => s,
        // 空脚本:交给旧包装(行为不变)
        None => return true,
    };
    if parts.next().is_some() {
        return false;
    }
    !starts_with_statement_keyword(first)
}

/// 片段是否以语句关键字开头(按标识符边界匹配,`letter` 不算 `let`)
fn starts_with_statement_keyword(segment: &str) -> bool {
    let head: String = segment
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect();
    STATEMENT_KEYWORDS.contains(&head.as_str())
}

/// `/` 是否应视为正则字面量起始(依据上一个有效字符,标准启发式)
fn regex_allowed(prev: Option<u8>) -> bool {
    match prev {
        // 脚本开头 / 这些字符之后出现 `/` = 正则;`/` 之后(str.replace(/a/))同理
        None => true,
        Some(c) => matches!(
            c,
            b'(' | b'['
                | b'{'
                | b','
                | b';'
                | b':'
                | b'='
                | b'!'
                | b'&'
                | b'|'
                | b'?'
                | b'+'
                | b'-'
                | b'*'
                | b'%'
                | b'~'
                | b'^'
                | b'<'
                | b'>'
                | b'/'
        ),
    }
}

/// 跳过 `'...'` / `"..."`(支持反斜杠转义),返回结束位置(闭引号之后)
fn skip_quoted(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// 跳过模板字面量 `` `...` ``;`${}` 内部按「不扫描」处理(少切分只会退回旧行为)
fn skip_template(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// 跳过 `//` 行注释,返回换行符位置(不含)
fn skip_line_comment(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 2;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            return i;
        }
        i += 1;
    }
    bytes.len()
}

/// 跳过 `/* */` 块注释,返回 `*/` 之后的位置
fn skip_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 2;
    while i < bytes.len() {
        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

/// 跳过正则字面量 `/.../flags`,返回结束位置(含 flags 之后的第一个位置)
fn skip_regex(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    let mut in_class = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            // 换行说明这不是正则(容错:按当前位置收手,避免吞掉后续代码)
            b'\n' => return i,
            b'[' => {
                in_class = true;
                i += 1;
            }
            b']' => {
                in_class = false;
                i += 1;
            }
            b'/' if !in_class => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return j;
            }
            _ => i += 1,
        }
    }
    bytes.len()
}

/// 错误文案是否像「语法/解析错误」(跨平台)
///
/// - Linux(WebKitGTK):脚本抛异常时回调拿到空串 → `parse eval result: ...`;
/// - 其它路径可能直接带回引擎原文(`SyntaxError: Unexpected token ...`)。
fn is_syntax_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    [
        "syntaxerror",
        "unexpected token",
        "invalid or unexpected token",
        "unexpected end of input",
        "parse eval result",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

/// 超时/通道类失败:再试一次只会重复等待或重复失败,不值得回退
fn is_transient_failure(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("timeout") || lower.contains("not ready") || lower.contains("eval failed")
}

/// 获取可见文本
/// selector 为 None/空:取 document.body 全文;指定时取该元素 innerText(找不到返回明确错误)
pub fn visible_text(app: &AppHandle, selector: Option<&str>) -> ControlResult<String> {
    let js = match selector {
        Some(s) if !s.is_empty() => format!(
            r#"(function(){{
              const el = document.querySelector({s:?});
              if (!el) return {{error: "element not found"}};
              return el.innerText;
            }})()"#,
            s = s
        ),
        _ => "document.body ? document.body.innerText : ''".to_string(),
    };
    let v = eval(app, &js)?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    Ok(v.as_str().unwrap_or("").to_string())
}

/// 元素状态(可见性/是否禁用/尺寸);默认超时 15s
pub fn element_state(app: &AppHandle, selector: &str) -> ControlResult<Value> {
    element_state_with(app, selector, None)
}

/// 同 `element_state`,`timeout_ms` 为 Some 时该步 eval 用传入预算(ms)
pub fn element_state_with(
    app: &AppHandle,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Value> {
    eval_with(
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
        timeout_ms,
    )
}

/// 文档就绪状态快照 {readyState,url,title,timeOrigin}
///
/// `timeOrigin` 是**文档身份**信号:每次新文档加载都会变;SPA(`pushState`)不变(靠 URL 变化识别);
/// bfcache 恢复的是原文档对象,其 `timeOrigin` 仍是该文档创建时的值(与当前离开的文档不同)。
pub fn document_state(app: &AppHandle) -> ControlResult<Value> {
    eval(
        app,
        "(function(){return {readyState: document.readyState, url: location.href, title: document.title, timeOrigin: (window.performance && typeof performance.timeOrigin === \"number\") ? performance.timeOrigin : null};})()",
    )
}

/// 等待页面就绪:必须**确认发生了导航**(文档身份或 URL 变化)后才接受 `readyState>=interactive`
///
/// 导航判据(覆盖三类场景):
/// - 文档身份变化:`performance.timeOrigin` 与导航前不同 → 新文档加载 / 同 URL 重载;
/// - URL 变化 → SPA(`pushState`)/普通导航/bfcache 前进后退(恢复文档的 timeOrigin 不同);
/// 只有 `navigated && loaded` 才算就绪,避免把**旧文档的 complete** 当成就绪。
/// 保留有界超时:超时如实返回 `ready:false, timedOut:true`(+当前 `readyState`/`navigated`)。
pub fn wait_ready(
    app: &AppHandle,
    prev_url: &str,
    prev_time_origin: Option<f64>,
    timeout_ms: u64,
) -> ControlResult<Value> {
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_millis(timeout_ms);
    loop {
        let st = document_state(app)?;
        let rs = st
            .get("readyState")
            .and_then(Value::as_str)
            .unwrap_or("loading")
            .to_string();
        let url = st
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let title = st
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let to = st.get("timeOrigin").and_then(Value::as_f64);
        let url_changed = !prev_url.is_empty() && url != prev_url;
        let ident_changed = match (prev_time_origin, to) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        let navigated = url_changed || ident_changed;
        let loaded = rs == "interactive" || rs == "complete";
        if navigated && loaded {
            return Ok(json!({
                "ready": true, "readyState": rs, "url": url, "title": title,
                "navigated": true, "timedOut": false,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({
                "ready": false, "readyState": rs, "url": url, "title": title,
                "navigated": navigated, "timedOut": true,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
    }
}

/// 读取窗口滚动位置四舍五入值 (x, y)
/// `eval_timeout_ms` = Some 时该步 eval 用传入预算(ms),None 走默认 15s
fn scroll_pos(app: &AppHandle, eval_timeout_ms: Option<u64>) -> ControlResult<(i64, i64)> {
    let v = eval_with(
        app,
        "(function(){return {x: Math.round(window.scrollX), y: Math.round(window.scrollY)};})()",
        eval_timeout_ms,
    )?;
    Ok((
        v.get("x").and_then(Value::as_i64).unwrap_or(0),
        v.get("y").and_then(Value::as_i64).unwrap_or(0),
    ))
}

/// 滚动页面(dx/dy 为视口 CSS 像素偏移)并等待位置稳定
///
/// 滚动是异步的(尤其 `scroll-behavior: smooth`),发出后轮询窗口滚动位置,
/// 连续 `SCROLL_STABLE_SAMPLES` 次采样不变视为稳定;超时则如实返回 `stable:false`。
/// 返回 `{stable,timedOut,x,y,waitedMs}`
pub fn scroll(app: &AppHandle, dx: i32, dy: i32) -> ControlResult<Value> {
    let js = format!(
        r#"(function(){{ window.scrollBy({{left:{dx},top:{dy}}}); return {{ok:true}}; }})()"#
    );
    eval(app, &js)?;
    let timeout = scroll_timeout_ms();
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_millis(timeout);
    let mut last = scroll_pos(app, None)?;
    let mut stable = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        let cur = scroll_pos(app, None)?;
        if cur == last {
            stable += 1;
        } else {
            stable = 0;
            last = cur;
        }
        if stable >= SCROLL_STABLE_SAMPLES {
            return Ok(json!({
                "stable": true, "timedOut": false,
                "x": cur.0, "y": cur.1,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
        if std::time::Instant::now() >= deadline {
            return Ok(json!({
                "stable": false, "timedOut": true,
                "x": cur.0, "y": cur.1,
                "waitedMs": start.elapsed().as_millis() as u64,
            }));
        }
    }
}

/// 元素是否在视口内(有尺寸且与视口矩形相交)
fn element_in_viewport(
    app: &AppHandle,
    selector: &str,
    eval_timeout_ms: Option<u64>,
) -> ControlResult<bool> {
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var r = el.getBoundingClientRect();
          var vw = window.innerWidth, vh = window.innerHeight;
          var ok = r.width > 0 && r.height > 0 && r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw;
          return {{inViewport: !!ok}};
        }})()"#,
        sel = selector
    );
    let v = eval_with(app, &js, eval_timeout_ms)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    Ok(v.get("inViewport")
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

/// 兜底显式滚动后的有界等待(ms)
const FALLBACK_SCROLL_TIMEOUT_MS: u64 = 800;

/// 轮询等待元素进入视口且窗口滚动稳定;返回 `(reached, stable)`
/// `timeout_ms` = 本轮等待预算;`eval_timeout_ms` = 轮询内每步 eval 的预算(与 `--timeout` 同口径)
fn wait_element_in_view(
    app: &AppHandle,
    selector: &str,
    timeout_ms: u64,
    eval_timeout_ms: Option<u64>,
) -> ControlResult<(bool, bool)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut last = scroll_pos(app, eval_timeout_ms)?;
    let mut stable = 0u32;
    loop {
        let in_view = element_in_viewport(app, selector, eval_timeout_ms)?;
        if in_view && stable >= SCROLL_STABLE_SAMPLES {
            return Ok((true, true));
        }
        if std::time::Instant::now() >= deadline {
            return Ok((false, stable >= SCROLL_STABLE_SAMPLES));
        }
        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
        let cur = scroll_pos(app, eval_timeout_ms)?;
        if cur == last {
            stable += 1;
        } else {
            stable = 0;
            last = cur;
        }
    }
}

/// 兜底 JS:显式把元素在各级可滚动祖先(内层容器)居中,再对窗口 `scrollBy`
fn bring_ancestors_into_view_js(selector: &str) -> String {
    format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          var node = el.parentElement;
          while (node && node !== document.body && node !== document.documentElement) {{
            var cs = getComputedStyle(node);
            var oy = cs.overflowY;
            if ((oy === "auto" || oy === "scroll" || oy === "overlay") &&
                node.scrollHeight > node.clientHeight + 1) {{
              var r = el.getBoundingClientRect(), nr = node.getBoundingClientRect();
              node.scrollTop += (r.top - nr.top) - (node.clientHeight - r.height) / 2;
            }}
            node = node.parentElement;
          }}
          var r = el.getBoundingClientRect();
          window.scrollBy(0, r.top - (window.innerHeight - r.height) / 2);
          return {{ok:true}};
        }})()"#,
        sel = selector
    )
}

/// 滚动到元素并等待其真正进入视口且滚动位置稳定
///
/// 主路径用**立即**(非 smooth)的 `scrollIntoView({block:center,inline:center})`,一次性处理元素的
/// 所有滚动祖先与窗口(实测平滑滚动在**嵌套滚动容器**上不可靠:窗口可能停在中途,元素仍在视口外);
/// 若仍未进入视口,再显式逐级滚动可滚动祖先 + 窗口。全程有界超时,超时如实返回 `reached:false`
/// 并给 `reason`。
pub fn scroll_to_element(app: &AppHandle, selector: &str) -> ControlResult<Value> {
    scroll_to_element_with(app, selector, None)
}

/// 同 `scroll_to_element`,`timeout_ms` 为 Some 时贯穿整条链路:每步 eval 用该预算,
/// 两轮等待预算取 `timeout_ms`(未传则沿用各自默认 `scroll_timeout_ms()` / `FALLBACK_SCROLL_TIMEOUT_MS`)
/// 最坏总耗时 ≈ 每步预算 × 步数(2 轮等待 + 若干 eval)
pub fn scroll_to_element_with(
    app: &AppHandle,
    selector: &str,
    timeout_ms: Option<u64>,
) -> ControlResult<Value> {
    let start = std::time::Instant::now();
    let js = format!(
        r#"(function(){{
          var el = document.querySelector({sel:?});
          if (!el) return {{error: "element not found"}};
          el.scrollIntoView({{block:"center", inline:"center"}});
          return {{ok:true}};
        }})()"#,
        sel = selector
    );
    let v = eval_with(app, &js, timeout_ms)?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    let wait_ms = timeout_ms.unwrap_or_else(scroll_timeout_ms);
    let (reached, stable) = wait_element_in_view(app, selector, wait_ms, timeout_ms)?;
    if reached {
        return Ok(json!({
            "reached": true, "inViewport": true, "stable": stable, "timedOut": false,
            "waitedMs": start.elapsed().as_millis() as u64,
        }));
    }
    // 兜底:显式滚动可滚动祖先链 + 窗口,再做一次有界等待
    let _ = eval_with(app, &bring_ancestors_into_view_js(selector), timeout_ms);
    let fallback_ms = timeout_ms.unwrap_or(FALLBACK_SCROLL_TIMEOUT_MS);
    let (reached2, stable2) = wait_element_in_view(app, selector, fallback_ms, timeout_ms)?;
    let in_view = element_in_viewport(app, selector, timeout_ms)?;
    let mut out = json!({
        "reached": reached2, "inViewport": in_view, "stable": stable2, "timedOut": !reached2,
        "waitedMs": start.elapsed().as_millis() as u64,
    });
    if !reached2 {
        out["reason"] = json!(
            "element not brought into view (possibly outside the document's scrollable range)"
        );
    }
    Ok(out)
}

/// 等待元素出现(轮询,最多 timeout_ms)
/// 轮询内每步 eval 的预算取 `min(timeout_ms, EVAL_TIMEOUT_MS)`:默认(均 15s)时仍是 15s(与既有行为一致),
/// 模型传小 `--timeout` 时随之变小(快失败),不会被 eval 默认 15s 拖住
pub fn wait_for_selector(app: &AppHandle, selector: &str, timeout_ms: u64) -> ControlResult<bool> {
    let eval_ms = timeout_ms.min(crate::ui::EVAL_TIMEOUT_MS);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        // 单次轮询的 eval 预算不得超过**剩余**等待时间:否则最小预算(100ms)下,
        // 第一次 poll 的 eval 就吃满整份预算 → 直接 eval timeout,而不是如实返回 found=false。
        // 且 eval 失败不算致命:继续轮询直到耗尽 deadline(元素可能在下次 poll 才出现)。
        let remaining = deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            return Ok(false);
        }
        let eval_ms = eval_ms.min(remaining);
        let v = eval_with(
            app,
            &format!("!!document.querySelector({sel:?})", sel = selector),
            Some(eval_ms),
        );
        match v {
            Ok(val) if val.as_bool().unwrap_or(false) => return Ok(true),
            // eval 失败(超时/解析)不立刻放弃:预算未尽就继续 poll
            Ok(_) | Err(_) => {}
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

#[cfg(test)]
mod tests {
    use super::{
        RetryOn, eval_plan, eval_wrap, find_unquoted_engine, is_single_expression, is_syntax_error,
        return_wrap, validate_selector,
    };

    /// 合法 CSS 必须放行(含属性选择器里的 `>>`、CSS 相邻/子代组合符)
    #[test]
    fn accepts_valid_css() {
        for sel in [
            "div > a.btn",
            "div a",
            "input[id=main]",
            "[data-x=\"a>>b\"]",
            "[data-x='a>>b']",
            "a[href$=\"text=foo\"]",
            ":has(> img)",
            "#main .item:first-child",
        ] {
            assert!(validate_selector(sel).is_ok(), "should accept: {sel}");
        }
    }

    /// 中段 `>>`(引号外)必须拒绝
    #[test]
    fn rejects_infix_chain_selector() {
        let err = validate_selector("div >> text=foo").expect_err("must reject");
        assert!(err.contains(">>"), "unexpected message: {err}");
        assert!(validate_selector("div.container>>span").is_err());
    }

    /// 中段 `text=` / `xpath=` / `:has-text(` 必须拒绝
    #[test]
    fn rejects_infix_engines() {
        assert!(validate_selector("div > a text=foo").is_err());
        assert!(validate_selector("div > a xpath=/html/body").is_err());
        assert!(validate_selector("div:has-text(\"foo\")").is_err());
    }

    /// 前缀式 Playwright 选择器仍拒绝;XPath 路径仍拒绝
    #[test]
    fn rejects_prefix_engines_and_xpath() {
        assert!(validate_selector("text=登录").is_err());
        assert!(validate_selector("css=div > a").is_err());
        assert!(validate_selector("//div[@id='a']").is_err());
    }

    /// 引号感知扫描的边界:引号内不命中、转义引号不提前结束引号
    #[test]
    fn unquoted_scan_respects_quotes() {
        assert_eq!(find_unquoted_engine("div[data-x=\"a>>b\"]"), None);
        assert_eq!(find_unquoted_engine("div[data-x='a>>b']"), None);
        assert_eq!(find_unquoted_engine("div >> span"), Some(">>"));
        assert_eq!(find_unquoted_engine("a[title=\"x\\\" >> y\"]"), None);
    }

    /// 单表达式:首试脚本与旧版逐字节一致(不碰 eval),且只在语法错误时才回退
    #[test]
    fn eval_plan_single_expression_keeps_legacy_wrap() {
        let plan = eval_plan("1+1");
        assert_eq!(plan.primary, "(function(){ return 1+1; })()");
        assert!(!plan.primary.contains("eval("), "单表达式不该用 eval");
        assert_eq!(plan.retry_on, RetryOn::SyntaxOnly);
        assert_eq!(
            plan.fallback.as_deref(),
            Some("(function(){ return eval(\"1+1\"); })()")
        );
        // 语法错误才回退;超时/通道失败直接返回旧错误
        assert!(plan.retry_on.matches("SyntaxError: Unexpected token ';'"));
        assert!(!plan.retry_on.matches("eval timeout (budget 15000ms)"));
    }

    /// 多语句:首试走 eval 包装(字符串字面量需 JSON 转义),失败回退旧包装
    #[test]
    fn eval_plan_multi_statement_uses_eval_wrap() {
        for script in [
            "const a = 1; a + 1",
            "console.log('a'); 2",
            "if (window.x) { y(); }",
            "a = 1; b = 2",
        ] {
            let plan = eval_plan(script);
            assert_eq!(plan.primary, eval_wrap(script), "should go eval: {script}");
            assert_eq!(plan.retry_on, RetryOn::NonTransient);
            assert_eq!(
                plan.fallback.as_deref(),
                Some(return_wrap(script).as_str()),
                "fallback must be legacy wrap: {script}"
            );
        }
        // 引号/换行/反斜杠必须转义,否则脚本被截断
        let plan = eval_plan("const s = \"a\\nb\"; s");
        assert!(
            plan.primary.contains(r#""const s = \"a\\nb\"; s""#),
            "unexpected literal: {}",
            plan.primary
        );
        // 超时/通道失败不回退(避免双倍等待)
        assert!(!RetryOn::NonTransient.matches("eval timeout (budget 800ms)"));
        assert!(!RetryOn::NonTransient.matches("page webview not ready"));
        assert!(RetryOn::NonTransient.matches("parse eval result: EOF"));
    }

    /// 单表达式判定:顶层 `;`/语句关键字才算多语句;字符串、正则、括号内部不算
    #[test]
    fn single_expression_detection_edges() {
        for script in [
            "1 + 1",
            "document.title",
            "el.click();",
            "items.map(x => x * 2)",
            "({a: 1}).a",
            "document.querySelector('a;b')",
            "x = /a;b/.test(y)",
            "JSON.stringify({n: __events.length, t: 'x;y'})",
        ] {
            assert!(is_single_expression(script), "should be single: {script}");
        }
        for script in [
            "const a = 1; a",
            "console.log('a'); 2",
            "if (a) { b(); }",
            "let x = 1",
            "for (let i = 0; i < 2; i++) c++",
            "a = 1; b = 2",
        ] {
            assert!(!is_single_expression(script), "should be multi: {script}");
        }
        // 平台错误文案识别(不改写原文,仅决定是否回退)
        assert!(is_syntax_error(
            "parse eval result: EOF while parsing a value"
        ));
        assert!(is_syntax_error("Unexpected token '}'"));
        assert!(!is_syntax_error("eval timeout (budget 15000ms)"));
        assert!(!is_syntax_error("page webview not ready"));
    }
}
