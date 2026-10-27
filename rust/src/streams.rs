//! 控制台/网络事件流缓冲(1.12.0)
//!
//! 契约(见 `.tmp/scratch/impl-112-contract.md`,文档由插件侧维护):
//! - 每个 page webview 一份"流",按 **tab id** 归属;缓冲在 Rust 侧,跨导航保留;
//! - 两路流各自有**字节预算**,超预算逐出最旧并 `dropped += 1`(禁止无界增长);
//! - 单条上限:console msg 4 KiB、network url 2 KiB、headers 合计 32 KiB;
//! - console 聚合:相邻同 `(level, msg)`(同 tab/phase/source 时)合并 `count/firstTs/lastTs`;
//! - network 聚合:`group=burst` 按 `method|url|status|initiator|phase` 且与上一条间隔 ≤
//!   `networkBurstWindowMs` 才合并;`group=key` 忽略 phase/间隔;`group=none` 逐条;
//! - 关闭标签释放该 tab 记录;关闭会话释放全部。
//!
//! 时间戳统一为 Unix 毫秒。`phase` 由 [`PhaseTracker`] 维护:
//! `load` / `interactive` / `action:<cmd>`(工具命令执行期间 + 3s 宽限,最新命令胜出、窗口不叠加)。
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

/// console 单条消息字节上限(4 KiB)
pub const CONSOLE_MSG_MAX: usize = 4 * 1024;
/// network URL 字节上限(2 KiB)
pub const NETWORK_URL_MAX: usize = 2 * 1024;
/// network headers 合计字节上限(32 KiB)
pub const NETWORK_HEADERS_MAX: usize = 32 * 1024;
/// 截断后缀(契约要求)
pub const TRUNC_SUFFIX: &str = "…[truncated]";
/// 静默来源注入上报单条上限(body ≤ 256 KiB,见 http.rs)
pub const CONSOLE_UPLOAD_MAX_BYTES: usize = 256 * 1024;
/// 注入上报限速:每 IP+tab 每秒允许的条目数
pub const CONSOLE_UPLOAD_RATE: u32 = 200;
/// 动作相位宽限(ms):命令结束后仍保留 `action:<cmd>` 的时间
pub const ACTION_GRACE_MS: u64 = 3_000;
/// 动作相位硬上限(ms):防命令异常挂死导致相位永久停留在 action
pub const ACTION_MAX_MS: u64 = 60_000;
/// 未完成网络请求的最大保留数(超限丢最旧,防长连接/WS 导致无界增长)
const MAX_PENDING_REQUESTS: usize = 4096;

/// 运行期配置(启动期由 `--flag`/env 确定,运行期只读)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamsConfig {
    /// console 流字节预算
    pub console_bytes: usize,
    /// network 流字节预算
    pub network_bytes: usize,
    /// 同 key 网络请求合并窗口(ms)
    pub burst_window_ms: u64,
    /// Linux console 注入猴补开关(Windows 走 CDP,默认关)
    pub console_inject: bool,
}

impl Default for StreamsConfig {
    fn default() -> Self {
        Self {
            console_bytes: 8 * 1024 * 1024,
            network_bytes: 8 * 1024 * 1024,
            burst_window_ms: 2_000,
            // 契约:Linux 默认开注入,Windows 默认关(避免与 CDP 双计)
            console_inject: cfg!(target_os = "linux"),
        }
    }
}

/// 配置单元:启动期写入一次;未写入时按 env + 平台默认初始化
static CONFIG: OnceLock<StreamsConfig> = OnceLock::new();

/// 写入配置(必须早于任何流使用;重复写入忽略)
pub fn configure(cfg: StreamsConfig) {
    let _ = CONFIG.set(cfg);
}

/// 读取配置(首次访问时按 env 解析,缺省走平台默认)
pub fn config() -> StreamsConfig {
    *CONFIG.get_or_init(config_from_env)
}

/// 从环境变量解析配置;非法值退回默认(不 panic)
pub fn config_from_env() -> StreamsConfig {
    let mut cfg = StreamsConfig::default();
    if let Some(v) = env_mb("BT_CONSOLE_BUFFER_MB") {
        cfg.console_bytes = v;
    }
    if let Some(v) = env_mb("BT_NETWORK_BUFFER_MB") {
        cfg.network_bytes = v;
    }
    if let Some(v) = env_u64("BT_NETWORK_BURST_MS") {
        cfg.burst_window_ms = v;
    }
    if let Some(v) = env_bool("BT_CONSOLE_INJECT") {
        cfg.console_inject = v;
    }
    cfg
}

/// 读取 MB 环境变量并转字节(最小值 64 KiB,避免 0/极小值把缓冲变成"永逐出")
fn env_mb(key: &str) -> Option<usize> {
    let n = std::env::var(key).ok()?.trim().parse::<u64>().ok()?;
    Some(((n as usize).saturating_mul(1024 * 1024)).max(64 * 1024))
}

/// 读取无符号整型环境变量
fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok()?.trim().parse::<u64>().ok()
}

/// 读取布尔环境变量(1/true/on/yes 为真,0/false/off/no 为假)
fn env_bool(key: &str) -> Option<bool> {
    let v = std::env::var(key).ok()?;
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// 当前 Unix 毫秒
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 按字节上限截断字符串(保证结果 ≤ max),截断时追加契约后缀
pub fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(TRUNC_SUFFIX.len());
    let mut end = keep.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + TRUNC_SUFFIX.len());
    out.push_str(&s[..end]);
    out.push_str(TRUNC_SUFFIX);
    out
}

/// console 事件来源
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleSource {
    /// Windows CDP(`Runtime.consoleAPICalled` / `Log.entryAdded` / `Runtime.exceptionThrown`)
    Cdp,
    /// Linux 页面注入猴补
    Injected,
    /// 平台原生通道
    Native,
}

impl ConsoleSource {
    /// 序列化名(`cdp|injected|native`)
    pub fn as_str(self) -> &'static str {
        match self {
            ConsoleSource::Cdp => "cdp",
            ConsoleSource::Injected => "injected",
            ConsoleSource::Native => "native",
        }
    }
}

/// 一条 console 事件(存储态;`count` 仅存在于查询输出)
#[derive(Debug, Clone)]
pub struct ConsoleEvent {
    pub level: String,
    pub msg: String,
    pub ts: u64,
    pub phase: String,
    pub source: ConsoleSource,
    pub stack: Option<String>,
    pub tab: u32,
    /// 是否为 `Runtime.exceptionThrown`(供 `type=exceptions` 过滤)
    pub is_exception: bool,
    /// 浏览器内部日志(CDP `Log.entryAdded` 的 `source=violation|deprecation`,
    /// 如 `[DOM] Password field ...`);默认不返回,`includeInternal` 才显示
    pub internal: bool,
}

impl ConsoleEvent {
    /// 估算占用字节(用于字节预算,允许略高估)
    fn size(&self) -> usize {
        32 + self.level.len()
            + self.msg.len()
            + self.phase.len()
            + self.source.as_str().len()
            + self.stack.as_ref().map(|s| s.len()).unwrap_or(0)
    }
}

/// 查询输出的 console 条目(契约字段)
#[derive(Debug, Clone)]
pub struct ConsoleItem {
    pub level: String,
    pub msg: String,
    pub count: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub phase: String,
    pub source: String,
    pub stack: Option<String>,
    pub tab: u32,
    pub is_exception: bool,
}

impl ConsoleItem {
    /// 序列化为契约 JSON(不输出内部 `is_exception`)
    fn to_json(&self) -> Value {
        let mut o = Map::new();
        o.insert("level".into(), json!(self.level));
        o.insert("msg".into(), json!(self.msg));
        o.insert("count".into(), json!(self.count));
        o.insert("firstTs".into(), json!(self.first_ts));
        o.insert("lastTs".into(), json!(self.last_ts));
        o.insert("phase".into(), json!(self.phase));
        o.insert("source".into(), json!(self.source));
        if let Some(s) = &self.stack {
            o.insert("stack".into(), json!(s));
        }
        o.insert("tab".into(), json!(self.tab));
        Value::Object(o)
    }
}

/// 一条 network 事件(存储态)
#[derive(Debug, Clone)]
pub struct NetworkEvent {
    pub method: String,
    pub url: String,
    pub status: i32,
    /// `doc|script|image|font|xhr|fetch|other`
    pub typ: String,
    pub initiator: String,
    pub duration_ms: u64,
    pub failure: Option<String>,
    pub phase: String,
    pub ts: u64,
    pub tab: u32,
    /// 请求头 + 响应头(默认读取时脱敏;存储态保留原值以支持 `revealSecrets`)
    pub headers: Vec<(String, String)>,
}

impl NetworkEvent {
    /// 估算占用字节
    fn size(&self) -> usize {
        64 + self.method.len()
            + self.url.len()
            + self.typ.len()
            + self.initiator.len()
            + self.failure.as_ref().map(|s| s.len()).unwrap_or(0)
            + self.phase.len()
            + self
                .headers
                .iter()
                .map(|(k, v)| k.len() + v.len() + 4)
                .sum::<usize>()
    }
}

/// 查询输出的 network 条目(契约字段)
#[derive(Debug, Clone)]
pub struct NetworkItem {
    pub method: String,
    pub url: String,
    pub status: i32,
    pub typ: String,
    pub initiator: String,
    pub duration_ms: u64,
    pub failure: Option<String>,
    pub phase: String,
    pub count: u64,
    pub first_ts: u64,
    pub last_ts: u64,
    pub tab: u32,
    pub headers: Vec<(String, String)>,
}

impl NetworkItem {
    /// 序列化为契约 JSON
    fn to_json(&self, include_headers: bool, reveal_secrets: bool) -> Value {
        let mut o = Map::new();
        o.insert("method".into(), json!(self.method));
        o.insert("url".into(), json!(self.url));
        o.insert("status".into(), json!(self.status));
        o.insert("type".into(), json!(self.typ));
        o.insert("initiator".into(), json!(self.initiator));
        o.insert("durationMs".into(), json!(self.duration_ms));
        if let Some(f) = &self.failure {
            o.insert("failure".into(), json!(f));
        }
        o.insert("phase".into(), json!(self.phase));
        o.insert("count".into(), json!(self.count));
        o.insert("firstTs".into(), json!(self.first_ts));
        o.insert("lastTs".into(), json!(self.last_ts));
        o.insert("tab".into(), json!(self.tab));
        if include_headers {
            o.insert(
                "headers".into(),
                headers_json(&self.headers, reveal_secrets),
            );
        }
        Value::Object(o)
    }
}

/// headers 输出:对象形态;`Authorization`/`Cookie`/`Set-Cookie` 默认 `<redacted>`
pub fn headers_json(headers: &[(String, String)], reveal_secrets: bool) -> Value {
    let mut m = Map::new();
    for (k, v) in headers {
        let secret = !reveal_secrets && is_secret_header(k);
        m.insert(
            k.clone(),
            json!(if secret { "<redacted>" } else { v.as_str() }),
        );
    }
    Value::Object(m)
}

/// CDP `Network.Initiator` → `"<type> <url>:<line>"`(契约)
///
/// CDP 的脚本发起者通常只在 `initiator.stack.callFrames[0]` 里带 `url/lineNumber`
/// (顶层 `url` 多为空);优先取首帧,回退顶层,两者都没有时按契约记 `other`。
pub fn initiator_text(init: Option<&Value>) -> String {
    let Some(init) = init else {
        return "other".into();
    };
    let typ = init.get("type").and_then(|t| t.as_str()).unwrap_or("other");
    let frame = init
        .get("stack")
        .and_then(|s| s.get("callFrames"))
        .and_then(|f| f.as_array())
        .and_then(|f| f.first());
    let frame_url = frame
        .and_then(|f| f.get("url"))
        .and_then(|u| u.as_str())
        .unwrap_or("");
    let (url, line) = if !frame_url.is_empty() {
        (
            frame_url,
            frame
                .and_then(|f| f.get("lineNumber"))
                .and_then(|l| l.as_i64())
                .unwrap_or(0),
        )
    } else {
        (
            init.get("url").and_then(|u| u.as_str()).unwrap_or(""),
            init.get("lineNumber")
                .and_then(|l| l.as_i64())
                .unwrap_or(0),
        )
    };
    if url.is_empty() {
        return "other".into();
    }
    format!("{typ} {url}:{line}")
}

/// 是否敏感请求/响应头(大小写不敏感)
fn is_secret_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "set-cookie"
    )
}

/// 单条 headers 上限裁剪:按 `k+v` 累计,超过 32 KiB 的后续头直接丢弃
pub fn cap_headers(mut headers: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut total = 0usize;
    let mut out = Vec::with_capacity(headers.len());
    for (k, v) in headers.drain(..) {
        let cost = k.len() + v.len() + 4;
        if total + cost > NETWORK_HEADERS_MAX {
            continue;
        }
        total += cost;
        out.push((k, v));
    }
    out
}

/// console 缓冲(单 tab)
#[derive(Debug)]
pub struct ConsoleBuffer {
    entries: VecDeque<ConsoleEvent>,
    bytes: usize,
    dropped: u64,
    budget: usize,
}

impl ConsoleBuffer {
    /// 按预算创建空缓冲
    pub fn new(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            budget,
        }
    }

    /// 写入一条(超预算逐出最旧;`dropped` 计逐出条数)
    pub fn push(&mut self, mut ev: ConsoleEvent) {
        ev.msg = truncate_bytes(&ev.msg, CONSOLE_MSG_MAX);
        if let Some(s) = ev.stack.as_mut() {
            *s = truncate_bytes(s, CONSOLE_MSG_MAX * 2);
        }
        let sz = ev.size();
        // 单条就超预算:保留该条(否则永远看不到),但先清空更旧的内容
        self.entries.push_back(ev);
        self.bytes += sz;
        while self.bytes > self.budget && self.entries.len() > 1 {
            if let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.size());
                self.dropped += 1;
            }
        }
    }

    /// 清空(保留 dropped 统计,便于观察丢弃量)
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// 保留字节数
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// 逐出计数
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 原始条目数(未聚合)
    pub fn raw_len(&self) -> usize {
        self.entries.len()
    }

    /// 最旧条目的时间戳(跨 tab 全局预算逐出时用于比较)
    fn front_ts(&self) -> Option<u64> {
        self.entries.front().map(|e| e.ts)
    }

    /// 逐出最旧一条并返回其占用字节(全局预算跨 tab 逐出用)
    fn pop_oldest(&mut self) -> Option<usize> {
        let old = self.entries.pop_front()?;
        let sz = old.size();
        self.bytes = self.bytes.saturating_sub(sz);
        self.dropped += 1;
        Some(sz)
    }

    /// 按过滤条件 + 聚合方式取条目(item 已聚合,未截断)
    pub fn query(&self, q: &ConsoleQuery) -> Vec<ConsoleItem> {
        let mut out: Vec<ConsoleItem> = Vec::new();
        for e in &self.entries {
            if !q.matches(e) {
                continue;
            }
            if q.group == "burst" {
                if let Some(last) = out.last_mut() {
                    if last.level == e.level
                        && last.msg == e.msg
                        && last.phase == e.phase
                        && last.source == e.source.as_str()
                        && last.tab == e.tab
                        && last.is_exception == e.is_exception
                    {
                        last.count += 1;
                        last.last_ts = e.ts;
                        if last.stack.is_none() {
                            last.stack = e.stack.clone();
                        }
                        continue;
                    }
                }
            }
            out.push(ConsoleItem {
                level: e.level.clone(),
                msg: e.msg.clone(),
                count: 1,
                first_ts: e.ts,
                last_ts: e.ts,
                phase: e.phase.clone(),
                source: e.source.as_str().to_string(),
                stack: e.stack.clone(),
                tab: e.tab,
                is_exception: e.is_exception,
            });
        }
        out
    }
}

/// network 缓冲(单 tab)
#[derive(Debug)]
pub struct NetworkBuffer {
    entries: VecDeque<NetworkEvent>,
    bytes: usize,
    dropped: u64,
    budget: usize,
}

impl NetworkBuffer {
    /// 按预算创建空缓冲
    pub fn new(budget: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            dropped: 0,
            budget,
        }
    }

    /// 写入一条(逐出最旧;单条超预算仍保留)
    pub fn push(&mut self, mut ev: NetworkEvent) {
        ev.url = truncate_bytes(&ev.url, NETWORK_URL_MAX);
        ev.headers = cap_headers(std::mem::take(&mut ev.headers));
        let sz = ev.size();
        self.entries.push_back(ev);
        self.bytes += sz;
        while self.bytes > self.budget && self.entries.len() > 1 {
            if let Some(old) = self.entries.pop_front() {
                self.bytes = self.bytes.saturating_sub(old.size());
                self.dropped += 1;
            }
        }
    }

    /// 清空
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    /// 保留字节数
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// 逐出计数
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 原始条目数
    pub fn raw_len(&self) -> usize {
        self.entries.len()
    }

    /// 最旧条目的时间戳(跨 tab 全局预算逐出时用于比较)
    fn front_ts(&self) -> Option<u64> {
        self.entries.front().map(|e| e.ts)
    }

    /// 逐出最旧一条并返回其占用字节(全局预算跨 tab 逐出用)
    fn pop_oldest(&mut self) -> Option<usize> {
        let old = self.entries.pop_front()?;
        let sz = old.size();
        self.bytes = self.bytes.saturating_sub(sz);
        self.dropped += 1;
        Some(sz)
    }

    /// 查找最近一条 URL 含 `pattern` 的请求(可选起点:`ts >= since`)
    ///
    /// 返回 `(url, status, ts)`;`ts` 用于跨 tab 取全局最近一条。
    pub fn find_latest(&self, pattern: &str, since: Option<u64>) -> Option<(&str, i32, u64)> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.url.contains(pattern) && since.map(|s| e.ts >= s).unwrap_or(true))
            .map(|e| (e.url.as_str(), e.status, e.ts))
    }

    /// 按过滤条件 + 聚合方式取条目
    pub fn query(&self, q: &NetworkQuery, window_ms: u64) -> Vec<NetworkItem> {
        let mut out: Vec<NetworkItem> = Vec::new();
        // 与 out 对齐的聚合 key(避免把 key 塞进输出结构)
        let mut keys: Vec<String> = Vec::new();
        for e in &self.entries {
            if !q.matches(e) {
                continue;
            }
            let key = network_group_key(e, &q.group);
            if let Some(key) = &key {
                if let Some(last) = out.last_mut() {
                    let last_key = keys.last().map(String::as_str).unwrap_or("");
                    let same = last_key == key
                        && match q.group.as_str() {
                            // burst:与上一条同 key 且间隔 ≤ 窗口
                            "burst" => e.ts.saturating_sub(last.last_ts) <= window_ms,
                            // key:忽略 phase 与间隔(phase 不参与 key)
                            _ => true,
                        };
                    if same {
                        last.count += 1;
                        last.last_ts = e.ts;
                        last.duration_ms = e.duration_ms;
                        if last.headers.is_empty() {
                            last.headers = e.headers.clone();
                        }
                        if last.failure.is_none() {
                            last.failure = e.failure.clone();
                        }
                        continue;
                    }
                }
            }
            out.push(NetworkItem {
                method: e.method.clone(),
                url: e.url.clone(),
                status: e.status,
                typ: e.typ.clone(),
                initiator: e.initiator.clone(),
                duration_ms: e.duration_ms,
                failure: e.failure.clone(),
                phase: e.phase.clone(),
                count: 1,
                first_ts: e.ts,
                last_ts: e.ts,
                tab: e.tab,
                headers: e.headers.clone(),
            });
            keys.push(key.unwrap_or_default());
        }
        out
    }
}

/// 计算聚合 key(`group=none` 返回 None 表示不聚合)
/// - burst:`tab|method|url|status|initiator|phase`,并额外要求间隔 ≤ 窗口
/// - key:`tab|method|url|status|initiator`(忽略 phase 与间隔)
fn network_group_key(ev: &NetworkEvent, group: &str) -> Option<String> {
    match group {
        "none" => None,
        "key" => Some(format!(
            "{}|{}|{}|{}|{}",
            ev.tab, ev.method, ev.url, ev.status, ev.initiator
        )),
        // 默认 burst
        _ => Some(format!(
            "{}|{}|{}|{}|{}|{}",
            ev.tab, ev.method, ev.url, ev.status, ev.initiator, ev.phase
        )),
    }
}

/// console 查询条件
#[derive(Debug, Clone)]
pub struct ConsoleQuery {
    pub typ: String,
    pub search: String,
    pub group: String,
    /// 相位过滤:空=不过滤;`load`/`interactive`/`action:<cmd>`/`action:*`
    pub phase: String,
    pub tab: Option<u32>,
    /// 是否包含浏览器内部日志(violation/deprecation);默认 false
    pub include_internal: bool,
}

impl ConsoleQuery {
    fn matches(&self, e: &ConsoleEvent) -> bool {
        if let Some(tab) = self.tab {
            if e.tab != tab {
                return false;
            }
        }
        if !phase_matches(&self.phase, &e.phase) {
            return false;
        }
        // 内部日志(如 `[DOM] Password field ...`)默认不可见,与 `--type` 取值无关
        if e.internal && !self.include_internal {
            return false;
        }
        let type_ok = match self.typ.as_str() {
            "all" => true,
            "exceptions" => e.is_exception,
            "warn" => e.level == "warn",
            other => e.level == other,
        };
        if !type_ok {
            return false;
        }
        if !self.search.is_empty() && !e.msg.contains(&self.search) {
            return false;
        }
        true
    }
}

/// network 查询条件
#[derive(Debug, Clone)]
pub struct NetworkQuery {
    pub typ: String,
    /// `2xx|3xx|4xx|5xx` 或具体数字(字符串);空表示不过滤
    pub status: String,
    pub failed: Option<bool>,
    pub search: String,
    pub group: String,
    /// 相位过滤:空=不过滤;`load`/`interactive`/`action:<cmd>`/`action:*`
    pub phase: String,
    pub tab: Option<u32>,
}

impl NetworkQuery {
    fn matches(&self, e: &NetworkEvent) -> bool {
        if let Some(tab) = self.tab {
            if e.tab != tab {
                return false;
            }
        }
        if !phase_matches(&self.phase, &e.phase) {
            return false;
        }
        if self.typ != "all" && e.typ != self.typ {
            return false;
        }
        if !status_matches(&self.status, e.status) {
            return false;
        }
        if let Some(failed) = self.failed {
            let is_failed = e.failure.is_some() || e.status == 0;
            if is_failed != failed {
                return false;
            }
        }
        if !self.search.is_empty() && !e.url.contains(&self.search) {
            return false;
        }
        true
    }
}

/// 校验 status 过滤值:空/`all` 合法(不过滤),`2xx|3xx|4xx|5xx` 或纯数字合法
///
/// 非法值由端点层直接报参数错误(不再静默返回全部记录)。
pub fn validate_status_filter(filter: &str) -> Result<(), String> {
    let f = filter.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("all") {
        return Ok(());
    }
    let lower = f.to_ascii_lowercase();
    if let Some(cls) = lower.strip_suffix("xx") {
        if matches!(cls, "2" | "3" | "4" | "5") {
            return Ok(());
        }
        return Err(format!(
            "invalid status: {filter} (2xx|3xx|4xx|5xx|<code>)"
        ));
    }
    // 「纯数字」:只接受 0-9(不允许 `-1`/`+200`),且须能装进 i32
    if !f.is_empty()
        && f.bytes().all(|b| b.is_ascii_digit())
        && f.parse::<i32>().is_ok()
    {
        return Ok(());
    }
    Err(format!(
        "invalid status: {filter} (2xx|3xx|4xx|5xx|<code>)"
    ))
}

/// 解析 `--phase` 过滤值:空=不过滤;`load`/`interactive`/`action:<cmd>`(支持 `action:*`)
pub fn parse_phase_filter(raw: &str) -> Result<String, String> {
    let f = raw.trim();
    if f.is_empty() {
        return Ok(String::new());
    }
    match f {
        "load" | "interactive" => Ok(f.to_string()),
        _ => match f.strip_prefix("action:") {
            Some(cmd) if !cmd.is_empty() => Ok(f.to_string()),
            _ => Err(format!(
                "invalid phase: {raw} (load|interactive|action:<cmd>)"
            )),
        },
    }
}

/// 相位过滤匹配:空=全过;`action:*` 匹配任意动作窗口;其余精确匹配
fn phase_matches(filter: &str, phase: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    if f == "action:*" {
        return phase.starts_with("action:");
    }
    f == phase
}

/// status 过滤:`""`/`all` 全过;`2xx` 区间;纯数字精确匹配;非法值一律不匹配
fn status_matches(filter: &str, status: i32) -> bool {
    let f = filter.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("all") {
        return true;
    }
    let lower = f.to_ascii_lowercase();
    if let Some(cls) = lower.strip_suffix("xx") {
        return cls
            .parse::<i32>()
            .map(|d| matches!(d, 2..=5) && status / 100 == d)
            .unwrap_or(false);
    }
    if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    f.parse::<i32>().map(|n| n == status).unwrap_or(false)
}

/// 相位基态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BasePhase {
    Load,
    Interactive,
}

impl BasePhase {
    fn as_str(self) -> &'static str {
        match self {
            BasePhase::Load => "load",
            BasePhase::Interactive => "interactive",
        }
    }
}

/// 单 tab 的相位状态
#[derive(Debug, Clone)]
struct PhaseState {
    base: BasePhase,
    action: Option<ActionWindow>,
    seq: u64,
}

/// 动作窗口(最新命令胜出)
#[derive(Debug, Clone)]
struct ActionWindow {
    cmd: String,
    seq: u64,
    until: u64,
}

/// 动作窗口句柄(begin 返回,end 时用于判定是否仍是最新)
#[derive(Debug, Clone, Copy)]
pub struct ActionHandle {
    pub tab: u32,
    pub seq: u64,
}

/// 相位状态机(按 tab;纯内存,可单测)
#[derive(Debug, Default)]
pub struct PhaseTracker {
    states: HashMap<u32, PhaseState>,
}

impl PhaseTracker {
    /// 导航开始 → base=load
    pub fn on_navigation(&mut self, tab: u32) {
        let st = self.states.entry(tab).or_insert(PhaseState {
            base: BasePhase::Load,
            action: None,
            seq: 0,
        });
        st.base = BasePhase::Load;
    }

    /// 加载完成(load/interactive)→ base=interactive
    pub fn on_load_finished(&mut self, tab: u32) {
        let st = self.states.entry(tab).or_insert(PhaseState {
            base: BasePhase::Load,
            action: None,
            seq: 0,
        });
        st.base = BasePhase::Interactive;
    }

    /// 命令开始:开 `action:<cmd>` 窗口;新命令覆盖旧命令(不叠加)
    pub fn begin_action(&mut self, tab: u32, cmd: &str, now: u64) -> ActionHandle {
        let st = self.states.entry(tab).or_insert(PhaseState {
            base: BasePhase::Load,
            action: None,
            seq: 0,
        });
        st.seq += 1;
        let seq = st.seq;
        st.action = Some(ActionWindow {
            cmd: cmd.to_string(),
            seq,
            until: now.saturating_add(ACTION_MAX_MS),
        });
        ActionHandle { tab, seq }
    }

    /// 命令结束:仅当仍是最新命令时,把窗口收窄为 "now + 3s"
    pub fn end_action(&mut self, handle: ActionHandle, now: u64) {
        if let Some(st) = self.states.get_mut(&handle.tab) {
            if let Some(a) = st.action.as_mut() {
                if a.seq == handle.seq {
                    a.until = now.saturating_add(ACTION_GRACE_MS);
                }
            }
        }
    }

    /// 当前相位:文档加载期间(`load`)恒为 `load`;`interactive` 后才看动作窗口
    ///
    /// 优先级:`load`(基态为加载中) > `action:<cmd>`(窗口内) > 基态。
    /// 加载期的资源/接口本就属于文档加载,被命令窗口覆盖会让归因失真
    /// (实测 `phase=action:navigate` 盖住整段文档加载)。
    pub fn current(&self, tab: u32, now: u64) -> String {
        if let Some(st) = self.states.get(&tab) {
            if st.base == BasePhase::Load {
                return BasePhase::Load.as_str().to_string();
            }
            if let Some(a) = &st.action {
                if now < a.until {
                    return format!("action:{}", a.cmd);
                }
            }
            return st.base.as_str().to_string();
        }
        BasePhase::Load.as_str().to_string()
    }

    /// 释放单 tab
    pub fn release(&mut self, tab: u32) {
        self.states.remove(&tab);
    }

    /// 释放全部
    pub fn clear(&mut self) {
        self.states.clear();
    }
}

/// 单 tab 的两路流
#[derive(Debug)]
struct TabStream {
    console: ConsoleBuffer,
    network: NetworkBuffer,
}

/// 待完成的网络请求(按 (tab, requestId) 关联;CDP requestWillBeSent 写入)
#[derive(Debug, Clone)]
pub struct PendingRequest {
    pub method: String,
    pub url: String,
    pub typ: String,
    pub initiator: String,
    pub start_ts: u64,
    pub headers: Vec<(String, String)>,
}

/// 限速窗口
#[derive(Debug, Clone, Copy)]
struct RateWindow {
    start_ms: u64,
    count: u32,
}

/// expect_response 的匹配起点(不清 network 流,只按 `ts >= since` 过滤)
#[derive(Debug, Clone)]
struct ExpectState {
    pattern: String,
    since: u64,
}

/// 全局存储
struct Store {
    tabs: HashMap<u32, TabStream>,
    phase: PhaseTracker,
    pending: HashMap<(u32, String), PendingRequest>,
    rates: HashMap<(IpAddr, u32), RateWindow>,
    config: StreamsConfig,
    /// expect_response 的起点(不清流;`assert_response` 只匹配此后的条目)
    expect: Option<ExpectState>,
}

impl Store {
    fn new(config: StreamsConfig) -> Self {
        Self {
            tabs: HashMap::new(),
            phase: PhaseTracker::default(),
            pending: HashMap::new(),
            rates: HashMap::new(),
            config,
            expect: None,
        }
    }

    /// 取/建 tab 流
    fn tab_mut(&mut self, tab: u32) -> &mut TabStream {
        let cfg = self.config;
        self.tabs.entry(tab).or_insert_with(|| TabStream {
            console: ConsoleBuffer::new(cfg.console_bytes),
            network: NetworkBuffer::new(cfg.network_bytes),
        })
    }

    /// 全局 console 预算:跨 tab 共享,超预算逐出全局最旧(单条超大仍保留)
    fn enforce_console_budget(&mut self) {
        let budget = self.config.console_bytes;
        let mut total: usize = self.tabs.values().map(|t| t.console.bytes()).sum();
        let mut count: usize = self.tabs.values().map(|t| t.console.raw_len()).sum();
        while total > budget && count > 1 {
            let victim = self
                .tabs
                .iter()
                .filter_map(|(id, t)| t.console.front_ts().map(|ts| (*id, ts)))
                .min_by_key(|(_, ts)| *ts)
                .map(|(id, _)| id);
            let Some(id) = victim else { break };
            let Some(removed) = self.tabs.get_mut(&id).and_then(|t| t.console.pop_oldest()) else {
                break;
            };
            total = total.saturating_sub(removed);
            count = count.saturating_sub(1);
        }
    }

    /// 全局 network 预算:跨 tab 共享,超预算逐出全局最旧(单条超大仍保留)
    fn enforce_network_budget(&mut self) {
        let budget = self.config.network_bytes;
        let mut total: usize = self.tabs.values().map(|t| t.network.bytes()).sum();
        let mut count: usize = self.tabs.values().map(|t| t.network.raw_len()).sum();
        while total > budget && count > 1 {
            let victim = self
                .tabs
                .iter()
                .filter_map(|(id, t)| t.network.front_ts().map(|ts| (*id, ts)))
                .min_by_key(|(_, ts)| *ts)
                .map(|(id, _)| id);
            let Some(id) = victim else { break };
            let Some(removed) = self.tabs.get_mut(&id).and_then(|t| t.network.pop_oldest()) else {
                break;
            };
            total = total.saturating_sub(removed);
            count = count.saturating_sub(1);
        }
    }
}

/// 全局存储单例
static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

/// 取全局存储(首次访问按配置初始化)
fn store() -> &'static Mutex<Store> {
    STORE.get_or_init(|| Mutex::new(Store::new(config())))
}

/// 写入 console 事件(panic-safe:锁中毒时恢复内部值)
pub fn push_console(ev: ConsoleEvent) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let phase = s.phase.current(ev.tab, now_ms());
    let tab = ev.tab;
    let mut ev = ev;
    ev.phase = phase;
    s.tab_mut(tab).console.push(ev);
    s.enforce_console_budget();
}

/// 写入 network 事件(phase 由存储侧按当前状态填充)
pub fn push_network(ev: NetworkEvent) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let phase = s.phase.current(ev.tab, now_ms());
    let tab = ev.tab;
    let mut ev = ev;
    ev.phase = phase;
    s.tab_mut(tab).network.push(ev);
    s.enforce_network_budget();
}

/// 记录网络请求开始(供 response/failed 补齐 duration/initiator)
/// 超过 [`MAX_PENDING_REQUESTS`] 时丢弃最旧的待完成项(长连接/WS 不会无界增长)
pub fn network_request_started(tab: u32, request_id: &str, pending: PendingRequest) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    if s.pending.len() >= MAX_PENDING_REQUESTS {
        if let Some(oldest) = s
            .pending
            .iter()
            .min_by_key(|(_, v)| v.start_ts)
            .map(|(k, _)| k.clone())
        {
            s.pending.remove(&oldest);
        }
    }
    s.pending.insert((tab, request_id.to_string()), pending);
}

/// 取出并移除待完成请求
pub fn take_pending_request(tab: u32, request_id: &str) -> Option<PendingRequest> {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.pending.remove(&(tab, request_id.to_string()))
}

/// 释放待完成请求(loadingFinished 清理)
pub fn drop_pending_request(tab: u32, request_id: &str) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.pending.remove(&(tab, request_id.to_string()));
}

/// 相位:导航开始
pub fn phase_navigation(tab: u32) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.phase.on_navigation(tab);
}

/// 相位:加载完成
pub fn phase_load_finished(tab: u32) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.phase.on_load_finished(tab);
}

/// 相位:命令开始
pub fn begin_action(tab: u32, cmd: &str) -> ActionHandle {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.phase.begin_action(tab, cmd, now_ms())
}

/// 相位:命令结束
pub fn end_action(handle: ActionHandle) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.phase.end_action(handle, now_ms());
}

/// 当前相位(供测试/调试)
pub fn current_phase(tab: u32) -> String {
    let s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.phase.current(tab, now_ms())
}

/// 释放某 tab 的缓冲与相位(关闭标签)
pub fn release_tab(tab: u32) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.tabs.remove(&tab);
    s.phase.release(tab);
    s.pending.retain(|(t, _), _| *t != tab);
    s.rates.retain(|(_, t), _| *t != tab);
}

/// 释放全部(关闭会话)
pub fn clear_all() {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.tabs.clear();
    s.phase.clear();
    s.pending.clear();
    s.rates.clear();
    s.expect = None;
}

/// 清空 console 流(tab=None 清全部)
pub fn clear_console(tab: Option<u32>) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    match tab {
        Some(t) => {
            if let Some(ts) = s.tabs.get_mut(&t) {
                ts.console.clear();
            }
        }
        None => {
            for ts in s.tabs.values_mut() {
                ts.console.clear();
            }
        }
    }
}

/// 清空 network 流(tab=None 清全部)
pub fn clear_network(tab: Option<u32>) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    match tab {
        Some(t) => {
            if let Some(ts) = s.tabs.get_mut(&t) {
                ts.network.clear();
            }
        }
        None => {
            for ts in s.tabs.values_mut() {
                ts.network.clear();
            }
        }
    }
}

/// 查询 console 流,返回契约 `data`
pub fn query_console(q: &ConsoleQuery, limit: usize, clear: bool) -> Value {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let tabs: Vec<u32> = match q.tab {
        Some(t) => vec![t],
        None => s.tabs.keys().copied().collect(),
    };
    let mut items: Vec<ConsoleItem> = Vec::new();
    let mut dropped = 0u64;
    let mut bytes = 0usize;
    for t in tabs {
        if let Some(ts) = s.tabs.get(&t) {
            items.extend(ts.console.query(q));
            dropped += ts.console.dropped();
            bytes += ts.console.bytes();
        }
    }
    // 跨 tab 按时间排序(单 tab 本身有序)
    items.sort_by_key(|i| i.first_ts);
    let total = items.len();
    let start = total.saturating_sub(limit);
    let items: Vec<ConsoleItem> = items[start..].to_vec();
    let logs: Vec<String> = items
        .iter()
        .map(|i| format!("[{}] {}", i.level, i.msg))
        .collect();
    if clear {
        match q.tab {
            Some(t) => {
                if let Some(ts) = s.tabs.get_mut(&t) {
                    ts.console.clear();
                }
            }
            None => {
                for ts in s.tabs.values_mut() {
                    ts.console.clear();
                }
            }
        }
    }
    json!({
        "items": items.iter().map(ConsoleItem::to_json).collect::<Vec<_>>(),
        // 向后兼容:旧插件/模型按 logs 读取
        "logs": logs,
        "dropped": dropped,
        "bytes": bytes,
        "total": total,
        "limit": limit,
        "type": q.typ,
        "search": q.search,
        "group": q.group,
        "phase": q.phase,
        "clear": clear,
    })
}

/// 查询 network 流,返回契约 `data`
pub fn query_network(
    q: &NetworkQuery,
    limit: usize,
    include_headers: bool,
    reveal_secrets: bool,
    clear: bool,
) -> Value {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let window = s.config.burst_window_ms;
    let tabs: Vec<u32> = match q.tab {
        Some(t) => vec![t],
        None => s.tabs.keys().copied().collect(),
    };
    let mut items: Vec<NetworkItem> = Vec::new();
    let mut dropped = 0u64;
    let mut bytes = 0usize;
    for t in tabs {
        if let Some(ts) = s.tabs.get(&t) {
            items.extend(ts.network.query(q, window));
            dropped += ts.network.dropped();
            bytes += ts.network.bytes();
        }
    }
    items.sort_by_key(|i| i.first_ts);
    let total = items.len();
    let start = total.saturating_sub(limit);
    let items: Vec<NetworkItem> = items[start..].to_vec();
    if clear {
        match q.tab {
            Some(t) => {
                if let Some(ts) = s.tabs.get_mut(&t) {
                    ts.network.clear();
                }
            }
            None => {
                for ts in s.tabs.values_mut() {
                    ts.network.clear();
                }
            }
        }
    }
    json!({
        "items": items
            .iter()
            .map(|i| i.to_json(include_headers, reveal_secrets))
            .collect::<Vec<_>>(),
        "dropped": dropped,
        "bytes": bytes,
        "total": total,
        "limit": limit,
        "group": q.group,
        "phase": q.phase,
    })
}

/// 记录 expect_response 的匹配起点(**不清空** network 流,保留 `network_logs` 历史)
///
/// 返回起点时间戳;`assert_response` 只匹配 `ts >= since` 的条目。
pub fn mark_expect(pattern: &str) -> u64 {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let since = now_ms();
    s.expect = Some(ExpectState {
        pattern: pattern.to_string(),
        since,
    });
    since
}

/// 当前 expect 起点(未调用 expect_response 时为 None)
pub fn expect_cursor() -> Option<u64> {
    let s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.expect.as_ref().map(|e| e.since)
}

/// 当前 expect 记录的模式(供 `assert_response` 未显式传 pattern 时回退)
pub fn expect_pattern() -> Option<String> {
    let s = store().lock().unwrap_or_else(|e| e.into_inner());
    s.expect.as_ref().map(|e| e.pattern.clone())
}

/// 在 network 流中查找最近一条 URL 含 `pattern` 的请求(expect/assert-response 兼容)
///
/// `since` 非空时只匹配该时间戳之后的条目。
pub fn find_network(pattern: &str, since: Option<u64>) -> Option<(String, i32)> {
    let s = store().lock().unwrap_or_else(|e| e.into_inner());
    let mut best: Option<(&str, i32, u64)> = None;
    for tab in s.tabs.values() {
        if let Some((url, status, ts)) = tab.network.find_latest(pattern, since) {
            if best.map(|(_, _, t)| ts > t).unwrap_or(true) {
                best = Some((url, status, ts));
            }
        }
    }
    best.map(|(url, status, _)| (url.to_string(), status))
}

/// 注入上报限速:返回 (接受条数, 丢弃条数);窗口 1s、每 IP+tab 200 条
pub fn rate_limit_console(ip: IpAddr, tab: u32, n: usize) -> (usize, usize) {
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let now = now_ms();
    let w = s.rates.entry((ip, tab)).or_insert(RateWindow {
        start_ms: now,
        count: 0,
    });
    if now.saturating_sub(w.start_ms) >= 1_000 {
        w.start_ms = now;
        w.count = 0;
    }
    let remaining = CONSOLE_UPLOAD_RATE.saturating_sub(w.count) as usize;
    let accepted = n.min(remaining);
    w.count = w.count.saturating_add(accepted as u32);
    (accepted, n - accepted)
}

/// 超限丢弃计入 dropped(不写事件)
pub fn note_console_dropped(tab: u32, n: usize) {
    if n == 0 {
        return;
    }
    let mut s = store().lock().unwrap_or_else(|e| e.into_inner());
    let buf = &mut s.tab_mut(tab).console;
    buf.dropped = buf.dropped.saturating_add(n as u64);
}

/// 把 CDP 的 console 类型归一为 `log|info|warn|error|debug`
pub fn normalize_console_level(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "warning" | "warn" => "warn".into(),
        "verbose" => "debug".into(),
        "error" | "assert" => "error".into(),
        "info" => "info".into(),
        "debug" => "debug".into(),
        // log / dir / table / trace / startGroup 等一律归 log
        _ => "log".into(),
    }
}

/// CDP `Network` 资源类型归一为契约 `type`
pub fn normalize_network_type(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "document" => "doc".into(),
        "xhr" => "xhr".into(),
        "fetch" => "fetch".into(),
        "script" => "script".into(),
        "image" => "image".into(),
        "font" => "font".into(),
        _ => "other".into(),
    }
}

/// Linux(WebKitGTK)mime → 契约 type 的近似映射(无法区分 xhr/fetch,统一 other)
pub fn network_type_from_mime(mime: &str) -> String {
    let m = mime.to_ascii_lowercase();
    if m.contains("text/html") || m.contains("application/xhtml") {
        "doc".into()
    } else if m.contains("javascript") || m.contains("ecmascript") {
        "script".into()
    } else if m.starts_with("image/") {
        "image".into()
    } else if m.starts_with("font/") || m.contains("woff") || m.contains("truetype") {
        "font".into()
    } else {
        "other".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ce(level: &str, msg: &str, ts: u64) -> ConsoleEvent {
        ConsoleEvent {
            level: level.into(),
            msg: msg.into(),
            ts,
            phase: "load".into(),
            source: ConsoleSource::Cdp,
            stack: None,
            tab: 1,
            is_exception: false,
            internal: false,
        }
    }

    fn ne(method: &str, url: &str, status: i32, ts: u64, phase: &str) -> NetworkEvent {
        NetworkEvent {
            method: method.into(),
            url: url.into(),
            status,
            typ: "xhr".into(),
            initiator: "script a.js:1".into(),
            duration_ms: 5,
            failure: None,
            phase: phase.into(),
            ts,
            tab: 1,
            headers: vec![],
        }
    }

    /// 超预算逐出最旧,并累加 dropped;保留的最新条目仍在
    #[test]
    fn console_buffer_evicts_oldest_and_counts_dropped() {
        let mut b = ConsoleBuffer::new(300);
        for i in 0..10 {
            b.push(ce("log", &format!("message-{i}"), i));
        }
        assert!(b.bytes() <= 300);
        assert!(b.dropped() >= 1);
        assert!(b.raw_len() < 10);
    }

    /// console 单条超 4 KiB 截断并带后缀
    #[test]
    fn console_message_is_truncated_to_4k() {
        let mut b = ConsoleBuffer::new(1024 * 1024);
        b.push(ce("log", &"x".repeat(CONSOLE_MSG_MAX * 2), 1));
        let out = b.query(&ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "burst".into(),
            phase: String::new(),
            tab: Some(1),
            include_internal: false,
        });
        assert_eq!(out.len(), 1);
        assert!(out[0].msg.len() <= CONSOLE_MSG_MAX);
        assert!(out[0].msg.ends_with(TRUNC_SUFFIX));
    }

    /// 相邻同 (level,msg) 合并 count/firstTs/lastTs;中间插入不同消息则不合并
    #[test]
    fn console_burst_merges_adjacent_identical() {
        let mut b = ConsoleBuffer::new(1024 * 1024);
        b.push(ce("log", "dup", 100));
        b.push(ce("log", "dup", 200));
        b.push(ce("warn", "other", 250));
        b.push(ce("log", "dup", 300));
        let out = b.query(&ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "burst".into(),
            phase: String::new(),
            tab: Some(1),
            include_internal: false,
        });
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].count, 2);
        assert_eq!(out[0].first_ts, 100);
        assert_eq!(out[0].last_ts, 200);
        assert_eq!(out[2].count, 1);
    }

    /// group=none 逐条,不合并
    #[test]
    fn console_group_none_keeps_each_entry() {
        let mut b = ConsoleBuffer::new(1024 * 1024);
        b.push(ce("log", "dup", 1));
        b.push(ce("log", "dup", 2));
        let out = b.query(&ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
            include_internal: false,
        });
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].count, 1);
    }

    /// 浏览器内部日志(violation/deprecation)默认不返回,include_internal 才可见
    #[test]
    fn console_internal_hidden_by_default() {
        let mut b = ConsoleBuffer::new(1024 * 1024);
        let mut ev = ce(
            "debug",
            "[DOM] Password field is not contained in a form",
            1,
        );
        ev.internal = true;
        b.push(ev);
        b.push(ce("error", "boom", 2));
        let base = ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
            include_internal: false,
        };
        let out = b.query(&base);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].msg, "boom");
        let all = b.query(&ConsoleQuery {
            include_internal: true,
            ..base
        });
        assert_eq!(all.len(), 2);
    }

    /// 截断按字节安全切分(多字节字符不被截半)
    #[test]
    fn truncate_respects_char_boundary() {
        let s = "中".repeat(3000);
        let t = truncate_bytes(&s, 4096);
        assert!(t.len() <= 4096);
        assert!(t.ends_with(TRUNC_SUFFIX));
    }

    /// network burst:同 key 且间隔 ≤ 窗口合并;超窗口另起一条
    #[test]
    fn network_burst_merges_within_window_only() {
        let mut b = NetworkBuffer::new(1024 * 1024);
        b.push(ne("GET", "https://a/x", 200, 1000, "load"));
        b.push(ne("GET", "https://a/x", 200, 1500, "load"));
        b.push(ne("GET", "https://a/x", 200, 5000, "load"));
        let q = NetworkQuery {
            typ: "all".into(),
            status: String::new(),
            failed: None,
            search: String::new(),
            group: "burst".into(),
            phase: String::new(),
            tab: Some(1),
        };
        let out = b.query(&q, 2000);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].count, 2);
        assert_eq!(out[1].count, 1);
        let none = b.query(
            &NetworkQuery {
                group: "none".into(),
                ..q.clone()
            },
            2000,
        );
        assert_eq!(none.len(), 3);
    }

    /// group=key:忽略 phase 与间隔,全部合并
    #[test]
    fn network_group_key_ignores_phase_and_gap() {
        let mut b = NetworkBuffer::new(1024 * 1024);
        b.push(ne("GET", "https://a/x", 200, 1000, "load"));
        b.push(ne("GET", "https://a/x", 200, 9000, "action:click"));
        let out = b.query(
            &NetworkQuery {
                typ: "all".into(),
                status: String::new(),
                failed: None,
                search: String::new(),
                group: "key".into(),
                phase: String::new(),
                tab: Some(1),
            },
            1,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].count, 2);
    }

    /// status 过滤:2xx / 数字 / failed
    #[test]
    fn network_filters_status_and_failure() {
        let mut b = NetworkBuffer::new(1024 * 1024);
        b.push(ne("GET", "https://a/ok", 204, 1, "load"));
        let mut failed = ne("GET", "https://a/bad", 0, 2, "load");
        failed.failure = Some("net::ERR_ABORTED".into());
        b.push(failed);
        let base = NetworkQuery {
            typ: "all".into(),
            status: "2xx".into(),
            failed: None,
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
        };
        assert_eq!(b.query(&base, 2000).len(), 1);
        let only_failed = b.query(
            &NetworkQuery {
                status: String::new(),
                failed: Some(true),
                ..base.clone()
            },
            2000,
        );
        assert_eq!(only_failed.len(), 1);
        assert_eq!(only_failed[0].status, 0);
    }

    /// headers 单条约 32 KiB 上限,超出的后续头被丢弃
    #[test]
    fn network_headers_capped() {
        let headers = (0..200)
            .map(|i| (format!("h{i}"), "v".repeat(1024)))
            .collect();
        let capped = cap_headers(headers);
        let total: usize = capped.iter().map(|(k, v)| k.len() + v.len() + 4).sum();
        assert!(total <= NETWORK_HEADERS_MAX);
        assert!(!capped.is_empty());
    }

    /// 敏感头默认脱敏,revealSecrets 才明文
    #[test]
    fn secret_headers_redacted_by_default() {
        let h = vec![
            ("Authorization".to_string(), "Bearer x".to_string()),
            ("Cookie".to_string(), "a=1".to_string()),
            ("X-Test".to_string(), "ok".to_string()),
        ];
        let v = headers_json(&h, false);
        assert_eq!(v["Authorization"], "<redacted>");
        assert_eq!(v["Cookie"], "<redacted>");
        assert_eq!(v["X-Test"], "ok");
        let v = headers_json(&h, true);
        assert_eq!(v["Authorization"], "Bearer x");
    }

    /// 相位:加载完成后命令窗口覆盖基态,结束后 3s 内仍是 action,过期回基态
    #[test]
    fn phase_action_window_and_grace() {
        let mut p = PhaseTracker::default();
        p.on_navigation(1);
        assert_eq!(p.current(1, 0), "load");
        p.on_load_finished(1);
        let h = p.begin_action(1, "click", 1000);
        assert_eq!(p.current(1, 1001), "action:click");
        p.end_action(h, 2000);
        assert_eq!(p.current(1, 4000), "action:click");
        assert_eq!(p.current(1, 6000), "interactive");
    }

    /// 相位优先级:文档加载期(load)恒为 load,不被 `action:<cmd>` 窗口覆盖
    #[test]
    fn phase_load_outranks_action_window() {
        let mut p = PhaseTracker::default();
        p.on_navigation(1);
        // 加载期 start_action:navigate(实测场景)→ 仍是 load
        let h = p.begin_action(1, "navigate", 1000);
        assert_eq!(p.current(1, 1001), "load");
        // 加载完成 → 窗口内生效为 action:navigate
        p.on_load_finished(1);
        assert_eq!(p.current(1, 1002), "action:navigate");
        p.end_action(h, 2000);
        assert_eq!(p.current(1, 9000), "interactive");
        // 再次导航回落 load,新命令也压不住
        p.on_navigation(1);
        let _ = p.begin_action(1, "click", 10_000);
        assert_eq!(p.current(1, 10_001), "load");
    }

    /// 并发命令取最新;旧命令 end 不影响新命令
    #[test]
    fn phase_latest_action_wins() {
        let mut p = PhaseTracker::default();
        p.on_load_finished(1);
        let h1 = p.begin_action(1, "click", 1000);
        let h2 = p.begin_action(1, "fill", 1100);
        assert_eq!(p.current(1, 1200), "action:fill");
        // 旧命令结束不覆盖新命令
        p.end_action(h1, 1300);
        assert_eq!(p.current(1, 1400), "action:fill");
        p.end_action(h2, 5000);
        assert_eq!(p.current(1, 6000), "action:fill");
        assert_eq!(p.current(1, 9000), "interactive");
    }

    /// 注入限速:单秒 200 条,超出丢弃;跨窗口重置
    #[test]
    fn console_upload_rate_limit() {
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        // 使用独立 tab 避免与其它测试共享全局窗口
        let tab = 9901;
        let (ok, drop) = rate_limit_console(ip, tab, 150);
        assert_eq!((ok, drop), (150, 0));
        let (ok, drop) = rate_limit_console(ip, tab, 100);
        assert_eq!((ok, drop), (50, 50));
    }

    /// 释放 tab 后该 tab 的缓冲不再可查(不泄漏)
    #[test]
    fn release_tab_frees_buffers() {
        let tab = 9902;
        push_console(ConsoleEvent {
            level: "log".into(),
            msg: "x".into(),
            ts: now_ms(),
            phase: String::new(),
            source: ConsoleSource::Cdp,
            stack: None,
            tab,
            is_exception: false,
            internal: false,
        });
        release_tab(tab);
        let q = ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "burst".into(),
            phase: String::new(),
            tab: Some(tab),
            include_internal: false,
        };
        let v = query_console(&q, 10, false);
        assert_eq!(v["total"], 0);
    }

    /// phase 过滤:load / interactive / action:<cmd> 精确匹配,action:* 通配动作窗口
    #[test]
    fn network_phase_filter_matches_exact_and_wildcard() {
        let mut b = NetworkBuffer::new(1024 * 1024);
        b.push(ne("GET", "https://a/load", 200, 1000, "load"));
        b.push(ne("GET", "https://a/act", 200, 2000, "action:click"));
        let base = NetworkQuery {
            typ: "all".into(),
            status: String::new(),
            failed: None,
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
        };
        for (phase, want) in [
            ("", 2usize),
            ("load", 1),
            ("interactive", 0),
            ("action:click", 1),
            ("action:fill", 0),
            ("action:*", 1),
        ] {
            let q = NetworkQuery {
                phase: phase.into(),
                ..base.clone()
            };
            assert_eq!(b.query(&q, 2000).len(), want, "phase={phase}");
        }
    }

    /// console phase 过滤同样生效
    #[test]
    fn console_phase_filter_matches() {
        let mut b = ConsoleBuffer::new(1024 * 1024);
        let mut load = ce("log", "during-load", 1);
        load.phase = "load".into();
        let mut act = ce("log", "during-click", 2);
        act.phase = "action:click".into();
        b.push(load);
        b.push(act);
        let base = ConsoleQuery {
            typ: "all".into(),
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
            include_internal: false,
        };
        assert_eq!(b.query(&base).len(), 2);
        assert_eq!(
            b.query(&ConsoleQuery {
                phase: "load".into(),
                ..base.clone()
            })
            .len(),
            1
        );
        assert_eq!(
            b.query(&ConsoleQuery {
                phase: "action:*".into(),
                ..base
            })
            .len(),
            1
        );
    }

    /// 非法 status 不再静默放行:validate 报错,matches 一律不匹配
    #[test]
    fn invalid_status_filter_is_rejected() {
        for bad in ["abc", "1xx", "2x", "20x", "-1", "600x"] {
            assert!(validate_status_filter(bad).is_err(), "status={bad}");
            assert!(!status_matches(bad, 200), "status={bad}");
        }
        for ok in ["", "all", "2xx", "3xx", "4xx", "5xx", "200", "0"] {
            assert!(validate_status_filter(ok).is_ok(), "status={ok}");
        }
        assert!(status_matches("2xx", 204));
        assert!(status_matches("200", 200));
        assert!(!status_matches("200", 201));
    }

    /// phase 解析:空/load/interactive/action:<cmd> 合法,其余报错(含空命令)
    #[test]
    fn phase_filter_parser_rejects_invalid() {
        for ok in ["", "load", "interactive", "action:click", "action:*"] {
            assert!(parse_phase_filter(ok).is_ok(), "phase={ok}");
        }
        for bad in ["action:", "loaded", "ACTION:click", "action"] {
            assert!(parse_phase_filter(bad).is_err(), "phase={bad}");
        }
    }

    /// initiator 优先取 stack.callFrames[0](fetch/xhr 常见),回退顶层 url
    #[test]
    fn initiator_prefers_stack_first_frame() {
        let stacked = json!({
            "type": "script",
            "stack": { "callFrames": [
                { "scriptId": "42", "url": "https://a/app.js", "lineNumber": 42 },
                { "scriptId": "1", "url": "https://a/vendor.js", "lineNumber": 9 }
            ] }
        });
        assert_eq!(
            initiator_text(Some(&stacked)),
            "script https://a/app.js:42"
        );
        let top = json!({ "type": "parser", "url": "https://a/doc", "lineNumber": 7 });
        assert_eq!(initiator_text(Some(&top)), "parser https://a/doc:7");
        assert_eq!(initiator_text(None), "other");
        let empty_stack = json!({ "type": "script", "stack": { "callFrames": [] } });
        assert_eq!(initiator_text(Some(&empty_stack)), "other");
    }

    /// expect 起点只做时间窗过滤:历史条目仍在,且只匹配起点之后的请求
    #[test]
    fn expect_cursor_filters_without_clearing_history() {
        let mut b = NetworkBuffer::new(1024 * 1024);
        b.push(ne("GET", "https://a/before", 200, 100, "load"));
        b.push(ne("GET", "https://a/after", 200, 300, "interactive"));
        // 历史未被清空
        let all = NetworkQuery {
            typ: "all".into(),
            status: String::new(),
            failed: None,
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(1),
        };
        assert_eq!(b.query(&all, 2000).len(), 2);
        // 起点 200:只剩 after
        let (url, status, _) = b.find_latest("https://a/", Some(200)).unwrap();
        assert_eq!(url, "https://a/after");
        assert_eq!(status, 200);
        // 起点之前无命中
        assert!(b.find_latest("https://a/", Some(301)).is_none());
    }

    /// 真实 mark_expect:不清全局 network 流,只记录游标
    #[test]
    fn mark_expect_does_not_clear_global_network_stream() {
        let tab = 9910;
        let mut ev = ne("GET", "https://expect/before", 200, now_ms(), "load");
        ev.tab = tab;
        push_network(ev);
        let q = NetworkQuery {
            typ: "all".into(),
            status: String::new(),
            failed: None,
            search: String::new(),
            group: "none".into(),
            phase: String::new(),
            tab: Some(tab),
        };
        let before = query_network(&q, 10, false, false, false)["total"]
            .as_u64()
            .unwrap();
        assert!(before >= 1);
        let since = mark_expect("https://expect/");
        assert_eq!(expect_cursor(), Some(since));
        let after = query_network(&q, 10, false, false, false)["total"]
            .as_u64()
            .unwrap();
        assert_eq!(after, before, "expect_response 不得清空 network 历史");
        release_tab(tab);
    }

    /// 预算口径:跨 tab 共享全局预算,超限逐出全局最旧
    #[test]
    fn stream_budget_is_global_across_tabs() {
        let mut st = Store::new(StreamsConfig {
            console_bytes: 400,
            network_bytes: 400,
            burst_window_ms: 2000,
            console_inject: false,
        });
        for i in 0..40u64 {
            let tab = if i % 2 == 0 { 1 } else { 2 };
            let mut ev = ce("log", &format!("message-{i}-{}", "x".repeat(40)), i * 10 + 1);
            ev.tab = tab;
            st.tab_mut(tab).console.push(ev);
            let mut net = ne("GET", &format!("https://a/{i}"), 200, i * 10 + 1, "load");
            net.tab = tab;
            st.tab_mut(tab).network.push(net);
            st.enforce_console_budget();
            st.enforce_network_budget();
        }
        let console_bytes: usize = st.tabs.values().map(|t| t.console.bytes()).sum();
        let network_bytes: usize = st.tabs.values().map(|t| t.network.bytes()).sum();
        assert!(console_bytes <= 400, "console={console_bytes}");
        assert!(network_bytes <= 400, "network={network_bytes}");
        // 两个 tab 都有保留(全局预算而非每 tab 预算)
        assert!(st.tabs[&1].console.raw_len() > 0);
        assert!(st.tabs[&2].console.raw_len() > 0);
    }
}
