//! 持久注入规则(1.12.0)
//!
//! 存储:`<sessionDataDir>/inject/rules.json`(sessionDataDir = `--user-data-dir`,
//! 即插件侧的 `sessions/<profile>/<sid>`;会话删除即随之清理)。
//!
//! - 规则模型:id / match / enabled / order / items / last / failStreak;
//! - 匹配:glob(`*` 通配、大小写不敏感)或 `re:<正则>`(非法正则按不匹配处理并记 `last.error`);
//! - 上限:规则 ≤ 20、单规则 items ≤ 256 KiB、全部 ≤ 1 MiB,超限返回明确错误;
//! - 连续失败 `failStreak >= 3` 自动 `enabled=false` 并保留原因;
//! - 写盘:临时文件 + rename 原子替换;读失败/坏 JSON → 空规则集 + 日志,不 panic。
//!
//! 执行时机见 [`on_page_load`]:页面加载完成(Windows `Page.loadEventFired` /
//! Linux `load-changed` FINISHED)后**在后台线程**串行执行该 tab 命中规则的 items,
//! 单条 15s 超时;执行期间不持规则写锁,也不碰 console/network 缓冲锁。
//! 同一 tab 同时只跑一批(per-label 轻量锁);失败只写 `last`,不向页面抛异常。
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::AppHandle;
use tauri::Manager;

/// 规则目录名(与 rules.json 一起位于 sessionDataDir 下)
const INJECT_DIR: &str = "inject";
/// 规则文件名
const RULES_FILE: &str = "rules.json";
/// 当前文件格式版本
const RULES_VERSION: u32 = 1;
/// 规则数上限
const MAX_RULES: usize = 20;
/// 单条规则 items 载荷上限(字节):`code` 合计(与插件侧同口径)
const MAX_RULE_ITEMS_BYTES: usize = 256 * 1024;
/// 全部规则 items 载荷上限(字节)
const MAX_TOTAL_ITEMS_BYTES: usize = 1024 * 1024;
/// 连续失败达到该值自动禁用
const AUTO_DISABLE_STREAK: u32 = 3;
/// 单条注入脚本的执行预算(与 `evaluate` 默认一致)
const ITEM_TIMEOUT_MS: u64 = 15_000;
/// `last.error` 保留的最大字符数(避免把页面长错误堆进 rules.json)
const MAX_ERROR_CHARS: usize = 300;

/// 规则写盘/读改写临界区(端点与后台执行共用;执行 items 期间**不持有**)
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// 正在执行注入批次的 webview label 集合(per-tab 防重入)
static RUNNING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

/// 一条规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// 规则 id(add 未提供时自动生成 `rN`)
    pub id: String,
    /// glob(`*` 通配)或 `re:<正则>`
    #[serde(rename = "match")]
    pub pattern: String,
    /// 是否参与匹配/执行
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 执行顺序(升序)
    #[serde(default)]
    pub order: i64,
    /// 注入项(目前只有 kind=text)
    #[serde(default)]
    pub items: Vec<Item>,
    /// 最近一次执行结果
    #[serde(default)]
    pub last: LastRun,
    /// 连续失败次数
    #[serde(default, rename = "failStreak")]
    pub fail_streak: u32,
}

impl Rule {
    /// 该规则 items 的载荷字节数(`code` 合计,与上限同口径)
    fn payload_bytes(&self) -> usize {
        items_bytes(&self.items)
    }
}

/// 单条注入项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// 注入类型(只支持 `text`)
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 待执行脚本
    #[serde(default)]
    pub code: String,
    /// 来源名(插件侧 basename,用于错误定位)
    #[serde(default)]
    pub name: String,
}

/// 最近一次执行结果
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LastRun {
    /// 执行时刻(毫秒时间戳)
    #[serde(default)]
    pub ts: u64,
    /// 成功条数
    #[serde(default)]
    pub ok: u32,
    /// 失败条数
    #[serde(default)]
    pub failed: u32,
    /// 最近一条错误(空串表示无)
    #[serde(default)]
    pub error: String,
}

/// rules.json 根对象
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesFile {
    /// 文件格式版本
    #[serde(default = "default_version")]
    pub version: u32,
    /// 规则列表(存储顺序;输出/执行前按 `order` 升序)
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Default for RulesFile {
    fn default() -> Self {
        Self {
            version: RULES_VERSION,
            rules: Vec::new(),
        }
    }
}

/// 单条 match 的判定结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchOutcome {
    /// 命中
    Hit,
    /// 未命中
    Miss,
    /// 模式非法(按不匹配处理,原因写 `last.error`)
    Invalid(String),
}

// ---------------------------------------------------------------------------
// 对外入口
// ---------------------------------------------------------------------------

/// 会话数据目录:优先 `--user-data-dir`,未提供时回落到应用本地数据目录。
///
/// 参数:`app` 应用句柄;返回规则文件所在会话目录(绝对路径,**不得**写进错误文案)
pub fn session_dir(app: &AppHandle) -> PathBuf {
    let custom = app
        .state::<crate::ui::UiState>()
        .user_data_dir
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match custom
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
    {
        Some(dir) => PathBuf::from(dir),
        // 未传 --user-data-dir:退回实例本地数据目录(多实例共用,属兼容路径)
        None => app
            .path()
            .app_local_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("bt-shell-inject")),
    }
}

/// `/api/inject/rules` 的纯逻辑实现(add/list/remove/clear/enable/disable)。
///
/// 参数:`dir` 会话数据目录;`body` 请求体
/// 返回:成功体 `{ rules: [...], rule?: {...} }`;失败为可读原因(不含绝对路径)
pub fn rules_action(dir: &Path, body: &Value) -> Result<Value, String> {
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let _guard = lock_config();
    match action.as_str() {
        "add" => action_add(dir, body),
        "list" => {
            let file = read_rules(dir);
            Ok(json!({ "rules": sorted_rules(&file.rules) }))
        }
        "remove" => action_remove(dir, body),
        "clear" => action_clear(dir),
        "enable" => action_set_enabled(dir, body, true),
        "disable" => action_set_enabled(dir, body, false),
        "" => Err("action is required (add/list/remove/clear/enable/disable)".into()),
        other => Err(format!(
            "unknown action: {other} (expected add/list/remove/clear/enable/disable)"
        )),
    }
}

/// `/api/inject/test` 的纯逻辑实现:**只匹配不执行**。
///
/// 参数:`dir` 会话数据目录;`url` 待匹配的页面地址
/// 返回:`{ matched: [id...], items: <命中规则 items 总数> }`
/// 说明:非法正则不计入 matched,并把原因写入该规则 `last.error`(属配置元数据,不是执行)。
pub fn test_match(dir: &Path, url: &str) -> Result<Value, String> {
    let _guard = lock_config();
    let mut file = read_rules(dir);
    let (hits, invalid) = select_rules(&file.rules, url);
    if !invalid.is_empty() {
        let mut changed = false;
        for (id, err) in &invalid {
            if let Some(rule) = file.rules.iter_mut().find(|r| &r.id == id) {
                rule.last.error = err.clone();
                changed = true;
            }
        }
        if changed {
            if let Err(e) = save_rules(dir, &file) {
                eprintln!("[inject] save rules failed: {e}");
            }
        }
    }
    let items: usize = hits.iter().map(|r| r.items.len()).sum();
    Ok(json!({
        "matched": hits.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        "items": items,
    }))
}

/// 页面加载完成回调:命中规则后**在后台线程**串行执行 items(不阻塞加载/UI 线程)。
///
/// 参数:`app` 应用句柄;`label` 刚加载完成的页面 webview label
/// 说明:同一 label 已有批次在跑时直接跳过(per-tab 防重入)。
pub fn on_page_load(app: &AppHandle, label: &str) {
    let Some(url) = tab_url(app, label) else {
        return;
    };
    if url.is_empty() || is_running(label) {
        return;
    }
    let app = app.clone();
    let label = label.to_string();
    std::thread::spawn(move || {
        let Some(guard) = RunGuard::acquire(&label) else {
            return;
        };
        run_batch(&app, &label, &url);
        drop(guard);
    });
}

// ---------------------------------------------------------------------------
// 匹配
// ---------------------------------------------------------------------------

/// 规则命中判定:`re:` 前缀走正则(区分大小写),其余走 glob(`*` 通配,大小写不敏感)。
pub fn rule_matches(pattern: &str, url: &str) -> MatchOutcome {
    match pattern.strip_prefix("re:") {
        Some(re) => match regex::Regex::new(re) {
            Ok(r) => {
                if r.is_match(url) {
                    MatchOutcome::Hit
                } else {
                    MatchOutcome::Miss
                }
            }
            Err(e) => {
                MatchOutcome::Invalid(truncate(&format!("invalid regex: {e}"), MAX_ERROR_CHARS))
            }
        },
        None => {
            if glob_match(pattern, url) {
                MatchOutcome::Hit
            } else {
                MatchOutcome::Miss
            }
        }
    }
}

/// glob 匹配:仅 `*` 有特殊含义(匹配任意长度,含空串),其余字符按字面量,大小写不敏感。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    // 最近一次 `*` 的位置与其已吞掉的文本长度(经典回溯法,无递归)
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] != '*' && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            // 让上一条 `*` 多吞一个字符后重试
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, ti));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

// ---------------------------------------------------------------------------
// 端点动作(均在 CONFIG_LOCK 内调用)
// ---------------------------------------------------------------------------

/// `action=add`:校验上限/生成 id/追加并原子写盘
fn action_add(dir: &Path, body: &Value) -> Result<Value, String> {
    let pattern = body
        .get("match")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    if pattern.is_empty() {
        return Err("match is required".into());
    }
    let items = parse_items(body)?;
    let order = match body.get("order") {
        None | Some(Value::Null) => 0,
        Some(v) => json_i64(v).ok_or_else(|| "order must be an integer".to_string())?,
    };
    let enabled = match body.get("enabled") {
        None | Some(Value::Null) => true,
        Some(v) => json_bool(v).ok_or_else(|| "enabled must be a boolean".to_string())?,
    };

    let mut file = read_rules(dir);
    if file.rules.len() >= MAX_RULES {
        return Err(format!("rule limit reached (max {MAX_RULES})"));
    }
    let rule_bytes = items_bytes(&items);
    if rule_bytes > MAX_RULE_ITEMS_BYTES {
        return Err(format!(
            "rule items too large: {rule_bytes} B exceeds 256 KiB limit"
        ));
    }
    let total = total_items_bytes(&file.rules) + rule_bytes;
    if total > MAX_TOTAL_ITEMS_BYTES {
        return Err(format!(
            "rules too large: {total} B exceeds 1 MiB total limit"
        ));
    }
    let id = resolve_id(body, &file.rules)?;
    let rule = Rule {
        id,
        pattern,
        enabled,
        order,
        items,
        last: LastRun::default(),
        fail_streak: 0,
    };
    file.rules.push(rule.clone());
    save_rules(dir, &file)?;
    Ok(json!({
        "rules": sorted_rules(&file.rules),
        "rule": rule,
    }))
}

/// `action=remove`:按 id 删除
fn action_remove(dir: &Path, body: &Value) -> Result<Value, String> {
    let id = required_id(body)?;
    let mut file = read_rules(dir);
    let Some(idx) = file.rules.iter().position(|r| r.id == id) else {
        return Err(format!("rule not found: {id}"));
    };
    file.rules.remove(idx);
    save_rules(dir, &file)?;
    Ok(json!({ "rules": sorted_rules(&file.rules) }))
}

/// `action=clear`:清空全部规则(保留空文件)
fn action_clear(dir: &Path) -> Result<Value, String> {
    let mut file = read_rules(dir);
    file.rules.clear();
    save_rules(dir, &file)?;
    Ok(json!({ "rules": Vec::<Rule>::new() }))
}

/// `action=enable|disable`:切换启用位;显式 enable 同时清零 `failStreak`
/// (否则自动禁用后一旦再失败一次就会立刻再次禁用,enable 形同无效)
fn action_set_enabled(dir: &Path, body: &Value, enabled: bool) -> Result<Value, String> {
    let id = required_id(body)?;
    let mut file = read_rules(dir);
    let Some(rule) = file.rules.iter_mut().find(|r| r.id == id) else {
        return Err(format!("rule not found: {id}"));
    };
    rule.enabled = enabled;
    if enabled {
        rule.fail_streak = 0;
    }
    save_rules(dir, &file)?;
    Ok(json!({ "rules": sorted_rules(&file.rules) }))
}

// ---------------------------------------------------------------------------
// 存储(原子写 / 坏文件回退)
// ---------------------------------------------------------------------------

/// 规则文件路径(不创建目录)
fn rules_path(dir: &Path) -> PathBuf {
    dir.join(INJECT_DIR).join(RULES_FILE)
}

/// 读取规则:不存在、读失败、JSON 损坏一律返回空规则集并记日志(不得崩)
fn read_rules(dir: &Path) -> RulesFile {
    let path = rules_path(dir);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RulesFile::default(),
        Err(e) => {
            eprintln!("[inject] read rules failed: {}", e.kind());
            return RulesFile::default();
        }
    };
    match serde_json::from_slice::<RulesFile>(&bytes) {
        Ok(mut f) => {
            if f.version == 0 {
                f.version = RULES_VERSION;
            }
            f
        }
        Err(e) => {
            eprintln!("[inject] rules.json is corrupt, starting empty: {e}");
            RulesFile::default()
        }
    }
}

/// 原子写规则:同目录临时文件 → rename 覆盖(Windows 的 rename 为替换语义)
fn save_rules(dir: &Path, file: &RulesFile) -> Result<(), String> {
    let target_dir = dir.join(INJECT_DIR);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("create inject dir failed: {}", e.kind()))?;
    let json =
        serde_json::to_vec_pretty(file).map_err(|e| format!("serialize rules failed: {e}"))?;
    let path = rules_path(dir);
    let tmp = target_dir.join(format!(
        "{RULES_FILE}.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    if let Err(e) = std::fs::write(&tmp, &json) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("write rules failed: {}", e.kind()));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("replace rules failed: {}", e.kind()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 执行
// ---------------------------------------------------------------------------

/// 一条规则的执行结果(合并写回时用)
struct RunOutcome {
    id: String,
    ok: u32,
    failed: u32,
    error: String,
}

/// 单批执行:读规则 → 匹配 → 串行执行 items → 合并写回 `last`/`failStreak`
fn run_batch(app: &AppHandle, label: &str, url: &str) {
    let dir = session_dir(app);
    let (selected, invalid) = {
        let _guard = lock_config();
        let file = read_rules(&dir);
        select_rules(&file.rules, url)
    };
    for (_, err) in &invalid {
        eprintln!("[inject] {err}");
    }
    if selected.is_empty() && invalid.is_empty() {
        return;
    }
    let Some(webview) = app.get_webview(label) else {
        return;
    };
    let mut outcomes: Vec<RunOutcome> = Vec::new();
    for rule in &selected {
        let (mut ok, mut failed) = (0u32, 0u32);
        let mut error = String::new();
        for item in &rule.items {
            match eval_item(&webview, &item.code) {
                Ok(()) => ok += 1,
                Err(e) => {
                    failed += 1;
                    let source = if item.name.is_empty() {
                        String::new()
                    } else {
                        format!("{}: ", item.name)
                    };
                    error = truncate(&format!("{source}{e}"), MAX_ERROR_CHARS);
                    eprintln!("[inject] rule {} item failed: {error}", rule.id);
                }
            }
        }
        if rule.items.is_empty() {
            continue;
        }
        outcomes.push(RunOutcome {
            id: rule.id.clone(),
            ok,
            failed,
            error,
        });
    }
    if outcomes.is_empty() && invalid.is_empty() {
        return;
    }
    // 合并写回:只取最新文件内容,避免覆盖期间端点做的增删改
    let _guard = lock_config();
    let mut file = read_rules(&dir);
    let ts = crate::streams::now_ms();
    let mut changed = false;
    for out in &outcomes {
        if let Some(rule) = file.rules.iter_mut().find(|r| r.id == out.id) {
            apply_outcome(rule, out, ts);
            changed = true;
        }
    }
    for (id, err) in &invalid {
        if let Some(rule) = file.rules.iter_mut().find(|r| r.id == *id) {
            rule.last.error = err.clone();
            changed = true;
        }
    }
    if changed {
        if let Err(e) = save_rules(&dir, &file) {
            eprintln!("[inject] save rules failed: {e}");
        }
    }
}

/// 把一次执行结果合并进规则:写 `last`;有失败累加 `failStreak`,达阈值自动禁用。
fn apply_outcome(rule: &mut Rule, out: &RunOutcome, ts: u64) {
    rule.last = LastRun {
        ts,
        ok: out.ok,
        failed: out.failed,
        error: out.error.clone(),
    };
    if out.failed > 0 {
        rule.fail_streak = rule.fail_streak.saturating_add(1);
        if rule.fail_streak >= AUTO_DISABLE_STREAK {
            rule.enabled = false;
            rule.last.error = format!(
                "auto-disabled after {} consecutive failed runs: {}",
                rule.fail_streak, out.error
            );
        }
    } else {
        rule.fail_streak = 0;
    }
}

/// 在指定 webview 执行一条注入脚本(15s 预算)
///
/// 脚本包在 try/catch 里:**异常不外抛**,统一以 `{ok,error}` 结果回传;
/// 语法错误会得到 `null` → 按失败处理。
fn eval_item(webview: &tauri::webview::Webview, code: &str) -> Result<(), String> {
    const PRELUDE: &str = "(function(){try{\n";
    const EPILOGUE: &str = "\n;return {ok:true};}catch(e){return {ok:false,error:String((e&&e.message)||e).slice(0,300)};}})()";
    let script = format!("{PRELUDE}{code}{EPILOGUE}");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    webview
        .eval_with_callback(&script, move |result| {
            let _ = tx.send(result);
        })
        .map_err(|e| format!("eval failed: {e}"))?;
    let raw = rx
        .recv_timeout(Duration::from_millis(ITEM_TIMEOUT_MS))
        .map_err(|_| format!("eval timeout (budget {ITEM_TIMEOUT_MS}ms)"))?;
    let value: Value = serde_json::from_str(raw.trim())
        .map_err(|_| "script did not return a result (likely syntax error)".to_string())?;
    if value.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        Ok(())
    } else {
        Err(value
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("script error")
            .to_string())
    }
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

/// per-tab 防重入守卫:构造即占位,Drop 释放(panic 时也能释放)
struct RunGuard {
    label: String,
}

impl RunGuard {
    /// 抢占 label;已被占用返回 `None`
    fn acquire(label: &str) -> Option<Self> {
        let running = RUNNING.get_or_init(|| Mutex::new(HashSet::new()));
        let mut set = running.lock().unwrap_or_else(|e| e.into_inner());
        if !set.insert(label.to_string()) {
            return None;
        }
        Some(Self {
            label: label.to_string(),
        })
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if let Some(running) = RUNNING.get() {
            let mut set = running.lock().unwrap_or_else(|e| e.into_inner());
            set.remove(&self.label);
        }
    }
}

/// label 是否已有批次在跑(仅用于避免无谓地起线程)
fn is_running(label: &str) -> bool {
    let running = RUNNING.get_or_init(|| Mutex::new(HashSet::new()));
    let set = running.lock().unwrap_or_else(|e| e.into_inner());
    set.contains(label)
}

/// 查看某 webview 对应标签的当前 URL(TabState 于导航时同步更新)
fn tab_url(app: &AppHandle, label: &str) -> Option<String> {
    let state = app.state::<crate::ui::UiState>();
    let tabs = state.tabs.lock().unwrap_or_else(|e| e.into_inner());
    tabs.iter()
        .find(|t| t.webview.as_deref() == Some(label))
        .map(|t| t.url.clone())
}

/// 取 CONFIG_LOCK(中毒则取回内部值,不因一次 panic 永久失效)
fn lock_config() -> std::sync::MutexGuard<'static, ()> {
    CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// 按 order 升序(同 order 按 id)输出规则
fn sorted_rules(rules: &[Rule]) -> Vec<Rule> {
    let mut out = rules.to_vec();
    out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    out
}

/// 选出命中的启用规则(按 order 升序)与非法模式
fn select_rules(rules: &[Rule], url: &str) -> (Vec<Rule>, Vec<(String, String)>) {
    let mut hits = Vec::new();
    let mut invalid = Vec::new();
    for rule in rules.iter().filter(|r| r.enabled) {
        match rule_matches(&rule.pattern, url) {
            MatchOutcome::Hit => hits.push(rule.clone()),
            MatchOutcome::Miss => {}
            MatchOutcome::Invalid(err) => invalid.push((rule.id.clone(), err)),
        }
    }
    hits.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    (hits, invalid)
}

/// 解析 items:只接受 kind=text;`code` 必填,`name` 可缺省
fn parse_items(body: &Value) -> Result<Vec<Item>, String> {
    let Some(raw) = body.get("items") else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let Some(list) = raw.as_array() else {
        return Err("items must be an array".into());
    };
    let mut items = Vec::with_capacity(list.len());
    for (idx, raw_item) in list.iter().enumerate() {
        let obj = raw_item
            .as_object()
            .ok_or_else(|| format!("items[{idx}] must be an object"))?;
        let kind = obj
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("text")
            .trim()
            .to_ascii_lowercase();
        if kind != "text" {
            return Err(format!(
                "items[{idx}]: unsupported kind: {kind} (only \"text\" is supported)"
            ));
        }
        let code = obj
            .get("code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("items[{idx}].code is required"))?
            .to_string();
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        items.push(Item {
            kind: "text".into(),
            code,
            name,
        });
    }
    Ok(items)
}

/// items 载荷字节数(仅 `code`:与插件侧 `Buffer.byteLength(code)` 同口径,
/// 避免恰好 256 KiB 的文件因附加 `name` 被误拒)
fn items_bytes(items: &[Item]) -> usize {
    items.iter().map(|i| i.code.len()).sum::<usize>()
}

/// 全部规则的 items 载荷字节数
fn total_items_bytes(rules: &[Rule]) -> usize {
    rules.iter().map(Rule::payload_bytes).sum()
}

/// 解析/生成规则 id
fn resolve_id(body: &Value, rules: &[Rule]) -> Result<String, String> {
    if let Some(id) = body
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if id.chars().count() > 64 || id.chars().any(char::is_control) {
            return Err("invalid id (max 64 chars, no control characters)".into());
        }
        if rules.iter().any(|r| r.id == id) {
            return Err(format!("rule id already exists: {id}"));
        }
        return Ok(id.to_string());
    }
    let mut n = rules.len() + 1;
    loop {
        let candidate = format!("r{n}");
        if !rules.iter().any(|r| r.id == candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// remove/enable/disable 必填 id
fn required_id(body: &Value) -> Result<String, String> {
    body.get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "id is required".to_string())
}

/// JSON 布尔(兼容命令行字符串 "true"/"1")
fn json_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        Value::Number(n) => n.as_i64().map(|n| n != 0),
        _ => None,
    }
}

/// JSON 整数(兼容命令行字符串)
fn json_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 截断到 `max` 个字符(按字符而非字节,避免切断多字节 UTF-8)
fn truncate(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// 缺省 id 版本
fn default_version() -> u32 {
    RULES_VERSION
}

/// 缺省启用状态
fn default_enabled() -> bool {
    true
}

/// 缺省 item 类型
fn default_kind() -> String {
    "text".into()
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个独立临时目录(用完由测试自行清理)
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("bt-inject-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 造一条 add 请求体
    fn add_body(id: &str, pattern: &str, code: &str) -> Value {
        json!({
            "action": "add",
            "id": id,
            "match": pattern,
            "items": [{ "kind": "text", "code": code, "name": "a.js" }],
        })
    }

    fn outcome(id: &str, ok: u32, failed: u32, error: &str) -> RunOutcome {
        RunOutcome {
            id: id.into(),
            ok,
            failed,
            error: error.into(),
        }
    }

    fn sample_rule() -> Rule {
        Rule {
            id: "r1".into(),
            pattern: "*host*".into(),
            enabled: true,
            order: 0,
            items: Vec::new(),
            last: LastRun::default(),
            fail_streak: 0,
        }
    }

    #[test]
    fn glob_matches_wildcard_case_insensitively() {
        assert!(glob_match("*ygpm.zhihjy.cn*", "https://A.YGPM.ZHIHJY.CN/x"));
        assert!(glob_match(
            "https://example.com/*",
            "HTTPS://EXAMPLE.COM/a/b"
        ));
        assert!(!glob_match("*foo*", "https://bar.example"));
        assert!(glob_match("**", "anything"));
        assert!(glob_match("plain", "PLAIN"));
        assert!(!glob_match("plain", "plainx"));
        assert!(glob_match("*a*b*c*", "xxAyyBzzC"));
        assert!(!glob_match("*a*b*c*", "xxAyyBzz"));
        assert!(glob_match("", ""));
        assert!(!glob_match("", "x"));
    }

    #[test]
    fn regex_match_and_invalid_pattern() {
        assert_eq!(
            rule_matches(r"re:^https://a\.com/", "https://a.com/x"),
            MatchOutcome::Hit
        );
        assert_eq!(
            rule_matches(r"re:^https://b\.com/", "https://a.com/x"),
            MatchOutcome::Miss
        );
        assert!(matches!(
            rule_matches("re:[", "https://a.com"),
            MatchOutcome::Invalid(_)
        ));
        // 非 `re:` 前缀按 glob 处理,不当正则
        assert_eq!(rule_matches("re", "re"), MatchOutcome::Hit);
    }

    #[test]
    fn add_list_remove_clear_roundtrip() {
        let dir = temp_dir("roundtrip");
        let added = rules_action(&dir, &add_body("r1", "*example.com*", "1+1")).unwrap();
        assert_eq!(added["rule"]["id"], "r1");
        assert_eq!(added["rule"]["enabled"], true);
        assert_eq!(added["rules"].as_array().unwrap().len(), 1);

        let listed = rules_action(&dir, &json!({ "action": "list" })).unwrap();
        assert_eq!(listed["rules"].as_array().unwrap().len(), 1);
        assert_eq!(listed["rules"][0]["match"], "*example.com*");

        let removed = rules_action(&dir, &json!({ "action": "remove", "id": "r1" })).unwrap();
        assert!(removed["rules"].as_array().unwrap().is_empty());
        assert!(
            rules_action(&dir, &json!({ "action": "remove", "id": "r1" }))
                .unwrap_err()
                .contains("rule not found")
        );

        rules_action(&dir, &add_body("r2", "*x*", "1")).unwrap();
        let cleared = rules_action(&dir, &json!({ "action": "clear" })).unwrap();
        assert!(cleared["rules"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_generates_id_and_rejects_duplicates() {
        let dir = temp_dir("ids");
        let first = rules_action(&dir, &add_body("", "*a*", "1")).unwrap();
        assert_eq!(first["rule"]["id"], "r1");
        let second = rules_action(&dir, &add_body("", "*b*", "1")).unwrap();
        assert_eq!(second["rule"]["id"], "r2");
        let dup = rules_action(&dir, &add_body("r1", "*c*", "1")).unwrap_err();
        assert!(dup.contains("already exists"), "{dup}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule_count_limit_is_enforced() {
        let dir = temp_dir("count");
        for i in 1..=MAX_RULES {
            rules_action(&dir, &add_body(&format!("r{i}"), "*x*", "1")).unwrap();
        }
        let err = rules_action(&dir, &add_body("r99", "*x*", "1")).unwrap_err();
        assert!(err.contains("max 20"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_rule_size_limit_is_enforced() {
        let dir = temp_dir("size");
        let code = "a".repeat(MAX_RULE_ITEMS_BYTES + 1);
        let err = rules_action(&dir, &add_body("r1", "*x*", &code)).unwrap_err();
        assert!(err.contains("256 KiB"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn total_size_limit_is_enforced() {
        let dir = temp_dir("total");
        // 每条 210 KiB(< 单条上限),第 5 条累计 1050 KiB > 1 MiB
        let code = "b".repeat(210 * 1024);
        for i in 1..=4 {
            rules_action(&dir, &add_body(&format!("r{i}"), "*x*", &code)).unwrap();
        }
        let err = rules_action(&dir, &add_body("r5", "*x*", &code)).unwrap_err();
        assert!(err.contains("1 MiB"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fail_streak_auto_disables_and_resets_on_success() {
        let mut rule = sample_rule();
        apply_outcome(&mut rule, &outcome("r1", 1, 1, "boom"), 1);
        assert_eq!(rule.fail_streak, 1);
        assert!(rule.enabled);
        apply_outcome(&mut rule, &outcome("r1", 0, 2, "boom"), 2);
        assert_eq!(rule.fail_streak, 2);
        assert!(rule.enabled);
        assert_eq!(rule.last.ok, 0);
        assert_eq!(rule.last.failed, 2);

        apply_outcome(&mut rule, &outcome("r1", 0, 1, "boom"), 3);
        assert_eq!(rule.fail_streak, 3);
        assert!(!rule.enabled);
        assert!(
            rule.last.error.contains("auto-disabled"),
            "{}",
            rule.last.error
        );

        // 成功一次清零 streak
        let mut rule2 = sample_rule();
        apply_outcome(&mut rule2, &outcome("r1", 0, 1, "boom"), 1);
        apply_outcome(&mut rule2, &outcome("r1", 2, 0, ""), 2);
        assert_eq!(rule2.fail_streak, 0);
        assert_eq!(rule2.last.ok, 2);
        assert_eq!(rule2.last.error, "");
    }

    #[test]
    fn enable_clears_fail_streak() {
        let dir = temp_dir("enable");
        rules_action(&dir, &add_body("r1", "*x*", "1")).unwrap();
        // 手工写入 3 连败 + 已禁用
        {
            let _g = lock_config();
            let mut file = read_rules(&dir);
            file.rules[0].enabled = false;
            file.rules[0].fail_streak = 3;
            save_rules(&dir, &file).unwrap();
        }
        let enabled = rules_action(&dir, &json!({ "action": "enable", "id": "r1" })).unwrap();
        assert_eq!(enabled["rules"][0]["enabled"], true);
        assert_eq!(enabled["rules"][0]["failStreak"], 0);
        let disabled = rules_action(&dir, &json!({ "action": "disable", "id": "r1" })).unwrap();
        assert_eq!(disabled["rules"][0]["enabled"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_leaves_no_temp_file_and_recovers_from_corrupt_json() {
        let dir = temp_dir("atomic");
        let body = add_body("r1", "*x*", "console.log(1)");
        rules_action(&dir, &body).unwrap();
        let path = rules_path(&dir);
        assert!(path.exists());
        // 同目录不应残留临时文件
        let leftovers: Vec<_> = std::fs::read_dir(dir.join(INJECT_DIR))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
        // 读到的是完整规则
        assert_eq!(read_rules(&dir).rules.len(), 1);

        // 坏 JSON → 空规则集(不 panic)
        std::fs::write(&path, b"{not json").unwrap();
        assert!(read_rules(&dir).rules.is_empty());
        // 且端点仍可继续写入
        assert_eq!(
            rules_action(&dir, &json!({ "action": "list" })).unwrap()["rules"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        rules_action(&dir, &add_body("r9", "*y*", "1")).unwrap();
        assert_eq!(read_rules(&dir).rules.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_endpoint_only_matches_enabled_rules() {
        let dir = temp_dir("match");
        rules_action(&dir, &add_body("r1", "*example.com*", "1")).unwrap();
        rules_action(&dir, &add_body("r2", "*other.com*", "1")).unwrap();
        rules_action(&dir, &json!({ "action": "disable", "id": "r2" })).unwrap();
        // r3:多 items,用于验证 items 计数
        rules_action(
            &dir,
            &json!({
                "action": "add", "id": "r3", "match": "re:example\\.com",
                "items": [
                    { "kind": "text", "code": "1", "name": "a.js" },
                    { "kind": "text", "code": "2", "name": "b.js" }
                ],
            }),
        )
        .unwrap();

        let res = test_match(&dir, "https://example.com/path").unwrap();
        assert_eq!(res["matched"], json!(["r1", "r3"]));
        assert_eq!(res["items"], 3);
        // 未命中任何规则
        let none = test_match(&dir, "https://nothing.test").unwrap();
        assert_eq!(none["matched"], json!([]));
        assert_eq!(none["items"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_endpoint_records_invalid_regex_without_match() {
        let dir = temp_dir("badregex");
        rules_action(&dir, &add_body("r1", "re:[", "1")).unwrap();
        let res = test_match(&dir, "https://example.com").unwrap();
        assert_eq!(res["matched"], json!([]));
        assert_eq!(res["items"], 0);
        let file = read_rules(&dir);
        assert!(
            file.rules[0].last.error.contains("invalid regex"),
            "{}",
            file.rules[0].last.error
        );
        assert!(file.rules[0].enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_rejects_unsupported_item_kind_and_bad_order() {
        let dir = temp_dir("validate");
        let err = rules_action(
            &dir,
            &json!({
                "action": "add", "id": "r1", "match": "*x*",
                "items": [{ "kind": "file", "code": "1" }],
            }),
        )
        .unwrap_err();
        assert!(err.contains("unsupported kind"), "{err}");

        let err = rules_action(
            &dir,
            &json!({ "action": "add", "id": "r2", "match": "*x*", "order": "abc" }),
        )
        .unwrap_err();
        assert!(err.contains("order must be an integer"), "{err}");

        let err = rules_action(&dir, &json!({ "action": "add" })).unwrap_err();
        assert!(err.contains("match is required"), "{err}");

        let err = rules_action(&dir, &json!({ "action": "nope" })).unwrap_err();
        assert!(err.contains("unknown action"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
