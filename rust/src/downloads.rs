//! 下载列表 + 每会话下载目录(1.13.0)
//!
//! 目录解析顺序(高 → 低):持久化文件 `<sessionDataDir>/download-dir.txt` →
//! 环境变量 `BT_DOWNLOAD_DIR` → 启动参数 `--download-dir` → 默认模板
//! `<cache>/sessions/<profile>/<sid>/downloads`。
//!
//! 记录口径:
//! - 每条记录归属「开始下载时」的目录,列表按目录分组;组按最近活动降序、组内按开始时间降序;
//! - 每组最多 `history_limit` 条(`BT_DOWNLOAD_HISTORY`,默认 64),**进行中不淘汰**;
//! - 取消按 id 处理:先调用采集层注册的取消钩子(Windows `DownloadOperation::Cancel`,
//!   Linux `WebKitDownload::cancel`),再尽力删除半成品文件。
//!
//! 记录持久化:
//! - 文件 `<sessionDataDir>/download-history.json`(会话数据目录根下,**不放**下载目录,
//!   避免用户清理下载文件时误删),内容为 `/api/downloads` 同形状的 item 数组;
//! - 一次性迁移:启动时新文件不存在而旧文件 `<sessionDataDir>/downloads/history.json`
//!   存在 → 读旧写新并删除旧文件(删除失败只记日志,不影响启动);
//! - 启动回填:上次进程被杀留下的 `progress` → `failed` + `error = "interrupted by restart"`;
//! - 变更后防抖落盘(≥[`HISTORY_DEBOUNCE_MS`] 静默才写;下载进度持续刷新、静默永不到来时
//!   按 [`HISTORY_MAX_WAIT_MS`] 上限兜底),退出前 `flush_history` 强制写;
//! - 原子写:先写 `download-history.json.tmp-<pid>` 再 rename 覆盖;坏 JSON 按空列表启动 + 日志。
//!
//! 采集:
//! - Windows:WebView2 `ICoreWebView2_4::DownloadStarting`,把 `ResultFilePath` 改到当前目录
//!   (重名自动加 ` (n)` 后缀),并订阅 `BytesReceivedChanged` / `StateChanged` 更新进度与状态;
//! - Windows 右键「图片/链接另存为」与「整页另存(Ctrl+S)」都走 `ICoreWebView2_25::SaveAsUIShowing`
//!   (**不走** `DownloadStarting`):置 `SuppressDefaultDialog(true)` 压掉系统对话框、
//!   `SaveAsFilePath` 指向当前目录(同 `unique_name` 去重)、`AllowReplace(false)`;
//!   该事件没有进度/完成回调,由后台线程等目标文件落盘(引擎先写临时文件、完成才改名)后置
//!   `completed`,超时置 `failed`;随后若 `DownloadStarting` 报同一路径,复用它而不是再建一条记录;
//! - 另存为按内容区分来源:`kind = "save-page"`(HTML 文档:整页另存)/ `"save-as"`(图片、链接、
//!   媒体等)。事件本身不说明「保存的是什么」,只能看 `ContentMimeType` 与建议文件名扩展名,
//!   HTML 系 MIME 或 `html/htm/mhtml/mht` 判为整页另存;判不出来的一律 `save-as`。
//!   已知不精确处:「链接另存为」指向 HTML 文档时会被判成 `save-page`;
//! - HTML 文档只能取 `HTML_ONLY`/`SINGLE_FILE`/`COMPLETE`(缺省 `DEFAULT` 不被接受),压制对话框后
//!   无法让用户选格式 → 统一按 `HTML_ONLY`(单文件)保存,图片等资源不随之落盘(见模块报告与 README);
//! - Linux:WebKitGTK `download-started` + `WebKitDownload`(`decide-destination` /
//!   `received-data` / `finished` / `failed`);**右键「图片另存为」/整页另存无对应事件**,
//!   仍弹系统对话框,故 Linux 不产生 `save-as` / `save-page` 记录;
//! - macOS:不支持(端点返回明确错误,不静默返回空列表)。
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use tauri::AppHandle;

/// 每组默认保留条数
pub const DEFAULT_HISTORY_LIMIT: usize = 64;
/// 历史上限环境变量
pub const HISTORY_ENV: &str = "BT_DOWNLOAD_HISTORY";
/// 下载目录覆盖环境变量
pub const DIR_ENV: &str = "BT_DOWNLOAD_DIR";
/// 目录持久化文件名(位于 `<sessionDataDir>` 内)
pub const DIR_FILE_NAME: &str = "download-dir.txt";
/// 默认目录模板
pub const DEFAULT_DIR_TEMPLATE: &str = "<cache>/sessions/<profile>/<sid>/downloads";
/// 派生不出 profile 时的兜底名
pub const DEFAULT_PROFILE: &str = "default";
/// 空/非法文件名兜底
const FALLBACK_FILENAME: &str = "download";
/// 重名后缀尝试次数上限
const MAX_NAME_ATTEMPTS: u32 = 1000;
/// 文件名/路径段长度上限(字符)
const MAX_SEGMENT_CHARS: usize = 120;
/// 面板事件名:新下载开始 / 状态变化(前端据此强制切视图并刷新列表)
pub const CHANGED_EVENT: &str = "downloads-changed";
/// 面板事件名:请求面板切到「下载」视图(payload `{}`;由 Tauri 命令 `panel_show_downloads` 广播)
pub const VIEW_EVENT: &str = "downloads-view-requested";
/// 记录来源:右键「另存为」(图片/链接等非 HTML 内容)
pub const SAVE_AS_KIND: &str = "save-as";
/// 记录来源:整页另存(`SaveAsUIShowing` 且内容为 HTML)
pub const SAVE_PAGE_KIND: &str = "save-page";
/// 删除进行中记录时的拒绝文案(`{delete}` 端点直接回给面板/模型)
pub const DELETE_PROGRESS_ERROR: &str = "正在下载，请先取消";
/// 记录历史文件名(位于 `<sessionDataDir>` 根下,**不放**下载目录,避免误删)
pub const HISTORY_FILE_NAME: &str = "download-history.json";
/// 旧版记录历史所在子目录(仅用于一次性迁移)
pub const LEGACY_HISTORY_SUBDIR: &str = "downloads";
/// 旧版记录历史文件名(仅用于一次性迁移)
pub const LEGACY_HISTORY_FILE_NAME: &str = "history.json";
/// 落盘防抖窗口:最后一次变更后至少静默这么久才写盘(多次变更合并成一次写)
pub const HISTORY_DEBOUNCE_MS: u64 = 500;
/// 防抖上限:持续变更(下载进度不断刷新)时最多等这么久就写一次,避免永远等不到静默
pub const HISTORY_MAX_WAIT_MS: u64 = 2000;
/// 后台落盘线程轮询间隔
const HISTORY_POLL_MS: u64 = 100;
/// 重启后仍为 `progress` 的记录写入的错误文案
pub const INTERRUPTED_ERROR: &str = "interrupted by restart";
/// 视为「整页另存」的 MIME(HTML 文档)
const HTML_MIMES: [&str; 2] = ["text/html", "application/xhtml+xml"];
/// 视为「整页另存」的扩展名
const HTML_EXTENSIONS: [&str; 4] = ["html", "htm", "mhtml", "mht"];

/// 下载状态
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Progress,
    Completed,
    Failed,
    Canceled,
}

impl DownloadState {
    /// 是否已结束(结束态才参与历史上限淘汰)
    fn is_terminal(self) -> bool {
        !matches!(self, DownloadState::Progress)
    }
}

/// `downloads-changed` 事件 payload(纯逻辑,便于单测)
///
/// 参数:`id` 记录 id;`filename` 仅「开始」事件传最终落盘文件名(非建议名),
/// 状态变化事件传 `None`(列表刷新按 id 定位);`state` 事件状态
/// 返回:「开始」`{id, filename, state: "progress"}`;状态变化 `{id, state}`
pub fn download_event_payload(id: &str, filename: Option<&str>, state: DownloadState) -> Value {
    match filename {
        Some(name) => json!({ "id": id, "filename": name, "state": state }),
        None => json!({ "id": id, "state": state }),
    }
}

/// 「下载完成」事件 payload(纯逻辑,便于单测)
///
/// 比其它结束态多带 `filename` / `path`:前端据此弹「已下载：<文件名>」提示
/// (另存为没有进度事件,也只能靠这里拿到最终文件名)。
pub fn download_completed_payload(id: &str, filename: &str, path: &str) -> Value {
    json!({
        "id": id,
        "filename": filename,
        "path": path,
        "state": DownloadState::Completed,
    })
}

/// 状态跃迁是否需要广播(纯逻辑,便于单测)
///
/// 只有「进行中 → 结束态」这一次需要广播:进度更新不广播,已结束不重复广播。
pub fn should_emit_state(prev: DownloadState, next: DownloadState) -> bool {
    prev == DownloadState::Progress && next.is_terminal()
}

/// 判定一次「另存为」的来源(纯逻辑,便于单测)
///
/// `SaveAsUIShowing` 不提供「保存的是什么」这一信息,只能按内容 MIME 与系统建议文件名
/// 判断:MIME 为 HTML 系(`text/html` / `application/xhtml+xml`)或建议名扩展名为
/// `html/htm/mhtml/mht` → 整页另存 [`SAVE_PAGE_KIND`];其余(图片/链接/音视频等)以及
/// 两者都没有信息时 → [`SAVE_AS_KIND`]。
///
/// 已知不精确处:「链接另存为」指向一个 HTML 文档时,与「整页另存」在事件层不可区分,
/// 统一按内容判为 `save-page`(见模块注释)。
pub fn classify_save_kind(mime: &str, suggested_path: &str) -> &'static str {
    // 先看建议文件名:WebView2 对「图片另存」也会回报 `text/html`(实测),
    // 只看 MIME 会把图片误判成整页另存,进而设成 HTML_ONLY 导致保存被拒。
    let ext = Path::new(suggested_path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if HTML_EXTENSIONS.contains(&ext.as_str()) {
        return SAVE_PAGE_KIND;
    }
    // 有扩展名且不是 HTML → 一律按资源另存(图片/链接/媒体)
    if !ext.is_empty() {
        return SAVE_AS_KIND;
    }
    // 没有扩展名时才退回 MIME 判断
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if HTML_MIMES.contains(&base.as_str()) {
        return SAVE_PAGE_KIND;
    }
    SAVE_AS_KIND
}

/// 单条下载记录(对外 JSON 形状:camelCase)
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub id: String,
    pub url: String,
    pub filename: String,
    pub path: String,
    pub total_bytes: u64,
    pub received_bytes: u64,
    pub state: DownloadState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 来源标记:普通下载为 `None`;另存为 [`SAVE_AS_KIND`] / [`SAVE_PAGE_KIND`]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub started_at: u64,
}

/// 取消钩子:采集层注册(WebView2 / WebKit 的取消调用)
pub type CancelFn = Box<dyn FnOnce() + Send + 'static>;

/// 内部记录:对外条目 + 分组目录 + 最近活动时间 + 取消钩子
struct Record {
    item: DownloadItem,
    dir: PathBuf,
    last_activity: u64,
    cancel: Option<CancelFn>,
}

/// 目录模板占位符取值
#[derive(Clone, Debug, Default)]
pub struct DirSpec {
    /// `<cache>`:插件缓存根
    pub cache_root: PathBuf,
    /// `<profile>`
    pub profile: String,
    /// `<sid>`
    pub sid: String,
    /// `<sname>`(会话名称)
    pub sname: String,
}

impl DirSpec {
    /// 从 `--user-data-dir`(`<cache>/sessions/<profile>/<sid>`)派生占位符取值
    ///
    /// 参数:`data_dir` 会话数据目录;`session_name` 会话名称(可为空,空则 `<sname>` 回退到 `<sid>`)
    /// 返回:占位符取值;路径不匹配 `sessions/<profile>/<sid>` 时 profile 取 `default`、cache 取家目录兜底
    pub fn for_data_dir(data_dir: Option<&str>, session_name: &str) -> Self {
        let (cache_root, profile, sid) = split_session_path(data_dir);
        Self {
            cache_root,
            profile,
            sid,
            sname: session_name.to_string(),
        }
    }
}

/// 启动配置(由 main 从命令行/env 组装)
pub struct StartupConfig {
    pub spec: DirSpec,
    pub session_data_dir: Option<PathBuf>,
    pub history_limit: usize,
    pub env_dir: Option<String>,
    pub arg_dir: Option<String>,
}

impl StartupConfig {
    /// 读取环境变量(`BT_DOWNLOAD_DIR` / `BT_DOWNLOAD_HISTORY`)并派生模板取值
    ///
    /// 参数:`data_dir` = `--user-data-dir`;`download_dir` = `--download-dir`;`session_name` = 会话名称
    pub fn from_args(
        data_dir: Option<&str>,
        download_dir: Option<&str>,
        session_name: &str,
    ) -> Self {
        let history_limit = std::env::var(HISTORY_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_HISTORY_LIMIT);
        Self {
            spec: DirSpec::for_data_dir(data_dir, session_name),
            session_data_dir: data_dir
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(PathBuf::from),
            history_limit,
            env_dir: std::env::var(DIR_ENV)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            arg_dir: download_dir
                .map(str::trim)
                .map(str::to_string)
                .filter(|v| !v.is_empty()),
        }
    }
}

/// 下载记录表(单会话一份;全局经 [`state`] 访问)
pub struct Downloads {
    dir: PathBuf,
    /// 模板展开的默认目录(不含运行时覆盖;`reset` 回到这里)
    default_dir: PathBuf,
    session_data_dir: Option<PathBuf>,
    history_limit: usize,
    records: Vec<Record>,
    next_id: u64,
    /// 记录历史文件(`<sessionDataDir>/download-history.json`);无会话数据目录时不落盘
    history_path: Option<PathBuf>,
    /// 内容代数:每次影响落盘内容的变更 +1
    history_rev: u64,
    /// 已落盘内容代数(等于 `history_rev` 表示无待写变更)
    history_saved_rev: u64,
    /// 最近一次变更时间(毫秒;防抖窗口起点)
    history_changed_at: u64,
    /// 本轮待写内容的起点(毫秒;防抖上限的计时起点)
    history_pending_since: u64,
}

impl Downloads {
    /// 新建记录表
    ///
    /// 参数:`dir` 初始下载目录;`session_data_dir` 持久化根;
    /// `history_limit` 每组上限(0 视为 1)
    ///
    /// `default_dir` 先取 `dir`,`init` 会用模板展开结果覆盖(见 [`Self::set_default_dir`]);
    /// 记录历史默认关闭,需 [`Self::set_history_path`] + [`Self::restore`] 打开
    pub fn new(dir: PathBuf, session_data_dir: Option<PathBuf>, history_limit: usize) -> Self {
        Self {
            default_dir: dir.clone(),
            dir,
            session_data_dir,
            history_limit: history_limit.max(1),
            records: Vec::new(),
            next_id: 0,
            history_path: None,
            history_rev: 0,
            history_saved_rev: 0,
            history_changed_at: 0,
            history_pending_since: 0,
        }
    }

    /// 设置默认目录(`reset` 的目标;应为模板展开结果,不含运行时覆盖)
    pub fn set_default_dir(&mut self, dir: PathBuf) {
        self.default_dir = dir;
    }

    /// 启用记录落盘(必须在 [`Self::restore`] 之前调用)
    pub fn set_history_path(&mut self, path: PathBuf) {
        self.history_path = Some(path);
        self.history_rev = 0;
        self.history_saved_rev = 0;
        self.history_changed_at = 0;
        self.history_pending_since = 0;
    }

    /// 记录历史文件路径(未启用时为 `None`)
    pub fn history_path(&self) -> Option<&Path> {
        self.history_path.as_deref()
    }

    /// 是否有待落盘的变更(无论是否已过防抖窗口)
    pub fn history_pending(&self) -> bool {
        self.history_path.is_some() && self.history_rev != self.history_saved_rev
    }

    /// 是否已到落盘时机(有待写变更,且「静默满防抖窗口」或「待写时长触顶」)
    ///
    /// 参数:`now` 当前毫秒时间戳(单测可传人造时间)
    pub fn history_due(&self, now: u64) -> bool {
        self.history_pending()
            && history_write_due(now, self.history_changed_at, self.history_pending_since)
    }

    /// 标记内容有变更(防抖窗口重新计时;未启用落盘时不做事)
    fn mark_dirty(&mut self) {
        if self.history_path.is_none() {
            return;
        }
        let now = now_ms();
        if self.history_rev == self.history_saved_rev {
            // 从「已落盘」进入「有待写」:本轮计时从这里开始
            self.history_pending_since = now;
        }
        self.history_rev += 1;
        self.history_changed_at = now;
    }

    /// 到点才落盘(后台线程每 [`HISTORY_POLL_MS`] 调用;单测可传人造时间)
    ///
    /// 返回:本次是否真的写了文件
    pub fn flush_if_due(&mut self, now: u64) -> bool {
        if !self.history_due(now) {
            return false;
        }
        self.flush_history()
    }

    /// 立即落盘(退出钩子 / 防抖到点调用);无待写变更时视为成功
    ///
    /// 返回:是否已把最新内容写入磁盘
    pub fn flush_history(&mut self) -> bool {
        let Some(path) = self.history_path.clone() else {
            return false;
        };
        if self.history_rev == self.history_saved_rev {
            return true;
        }
        let rev = self.history_rev;
        let items: Vec<&DownloadItem> = self.records.iter().map(|r| &r.item).collect();
        match write_history_file(&path, &items) {
            Ok(()) => {
                self.history_saved_rev = rev;
                true
            }
            Err(e) => {
                // 写盘失败只记日志:下载/列表功能不受影响,下次变更会重试
                eprintln!("[downloads] persist history failed: {e}");
                false
            }
        }
    }

    /// 从 `download-history.json` 回填记录(启动时调用一次)
    ///
    /// 规则:
    /// - 文件不存在 → 空列表(正常首启);
    /// - 文件不可读 / 不是合法 JSON / 顶层不是数组 → 空列表 + 日志,**不崩**;
    ///   单条形状不合法(缺 `id` 或 `path`)→ 跳过该条 + 日志;
    /// - 上次进程被杀留下的 `progress` → 置 `failed`,`error` = [`INTERRUPTED_ERROR`],
    ///   并标记待落盘(修正结果也要写回);
    /// - 分组目录取记录自身 `path` 的父目录;`last_activity` 取 `started_at`;
    ///   `next_id` 抬到已有 id 之后,避免重启后 id 撞车。
    pub fn restore(&mut self) {
        let Some(path) = self.history_path.clone() else {
            return;
        };
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                eprintln!("[downloads] read history failed: {e}; start with empty list");
                return;
            }
        };
        let parsed: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(e) => {
                eprintln!("[downloads] history file invalid ({e}); start with empty list");
                return;
            }
        };
        let Some(entries) = parsed.as_array() else {
            eprintln!("[downloads] history file is not an array; start with empty list");
            return;
        };
        let fallback_dir = self.dir.clone();
        let mut interrupted = 0usize;
        for value in entries {
            let Some(mut item) = item_from_json(value) else {
                eprintln!("[downloads] skip malformed history entry: {value}");
                continue;
            };
            if item.state == DownloadState::Progress {
                item.state = DownloadState::Failed;
                item.error = Some(INTERRUPTED_ERROR.to_string());
                interrupted += 1;
            }
            let dir = Path::new(&item.path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| fallback_dir.clone());
            if let Some(seq) = id_seq(&item.id) {
                self.next_id = self.next_id.max(seq.saturating_add(1));
            }
            self.records.push(Record {
                last_activity: item.started_at,
                dir,
                item,
                cancel: None,
            });
        }
        eprintln!(
            "[downloads] history loaded: {} entries ({interrupted} interrupted → failed)",
            self.records.len()
        );
        if interrupted > 0 {
            self.mark_dirty();
        }
    }

    /// 当前下载目录
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 默认下载目录(模板展开结果)
    pub fn default_dir(&self) -> &Path {
        &self.default_dir
    }

    /// 每组历史上限
    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// 登记一次下载开始,分配目标路径(重名自动加 ` (n)` 后缀)
    ///
    /// 返回:`(记录 id, 目标绝对路径)`;调用方负责把目标路径交给采集层(WebView2 `ResultFilePath`)
    pub fn begin(&mut self, url: &str, suggested_path: &str) -> (String, PathBuf) {
        self.begin_kind(url, suggested_path, None)
    }

    /// 同 [`Self::begin`],额外记录来源标记(`kind`:普通下载 `None`、另存为 [`SAVE_AS_KIND`])
    pub fn begin_kind(
        &mut self,
        url: &str,
        suggested_path: &str,
        kind: Option<&str>,
    ) -> (String, PathBuf) {
        let dir = self.dir.clone();
        let suggested = Path::new(suggested_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let filename = unique_name(&suggested, |candidate| {
            let path = dir.join(candidate);
            path.exists()
                || self.records.iter().any(|r| {
                    r.item.state == DownloadState::Progress && PathBuf::from(&r.item.path) == path
                })
        });
        let id = format!("dl-{}", self.next_id);
        self.next_id += 1;
        let path = dir.join(&filename);
        let now = tick();
        self.records.push(Record {
            item: DownloadItem {
                id: id.clone(),
                url: url.to_string(),
                filename,
                path: path.to_string_lossy().to_string(),
                total_bytes: 0,
                received_bytes: 0,
                state: DownloadState::Progress,
                error: None,
                kind: kind.map(str::to_string),
                started_at: now,
            },
            dir: dir.clone(),
            last_activity: now,
            cancel: None,
        });
        self.prune_dir(&dir);
        self.mark_dirty();
        (id, path)
    }

    /// 记录是否仍在进行中(采集层的等待线程据此提前收工)
    pub fn is_progress(&self, id: &str) -> bool {
        self.records
            .iter()
            .any(|r| r.item.id == id && r.item.state == DownloadState::Progress)
    }

    /// 注册取消钩子(采集层在拿到操作句柄后调用)
    pub fn set_cancel_hook(&mut self, id: &str, hook: CancelFn) {
        if let Some(rec) = self.records.iter_mut().find(|r| r.item.id == id) {
            rec.cancel = Some(hook);
        }
    }

    /// 更新进度(未知总量时 `total` 传 0)
    pub fn set_progress(&mut self, id: &str, received: u64, total: u64) {
        let mut changed = false;
        if let Some(rec) = self.records.iter_mut().find(|r| r.item.id == id) {
            if rec.item.state != DownloadState::Progress {
                return;
            }
            // 同值回调不重复标记变更(落盘防抖只管真实变化)
            changed =
                rec.item.received_bytes != received || (total > 0 && rec.item.total_bytes != total);
            rec.item.received_bytes = received;
            if total > 0 {
                rec.item.total_bytes = total;
            }
            rec.last_activity = tick();
        }
        if changed {
            self.mark_dirty();
        }
    }

    /// 写入结束状态(已结束的记录不会被后续回调覆盖;总量未知时用已收字节补全)
    ///
    /// 返回:是否发生了「进行中 → 结束态」的跃迁(调用方据此只广播一次)
    pub fn finish(&mut self, id: &str, state: DownloadState, error: Option<String>) -> bool {
        let Some(rec) = self.records.iter_mut().find(|r| r.item.id == id) else {
            return false;
        };
        if !should_emit_state(rec.item.state, state) {
            return false;
        }
        rec.item.state = state;
        rec.item.error = error;
        if rec.item.total_bytes == 0 && rec.item.received_bytes > 0 {
            rec.item.total_bytes = rec.item.received_bytes;
        }
        rec.last_activity = tick();
        let dir = rec.dir.clone();
        self.prune_dir(&dir);
        self.mark_dirty();
        true
    }

    /// 取消进行中的下载:标记 canceled → 调用采集层取消钩子 → 尽力删除半成品
    ///
    /// 错误文案不含路径细节(端点可直接回给模型)
    pub fn cancel(&mut self, id: &str) -> Result<(), String> {
        let (path, hook, dir) = {
            let Some(rec) = self.records.iter_mut().find(|r| r.item.id == id) else {
                return Err(format!("download not found: {id}"));
            };
            if rec.item.state != DownloadState::Progress {
                return Err("download is not in progress".to_string());
            }
            rec.item.state = DownloadState::Canceled;
            rec.item.error = None;
            rec.last_activity = tick();
            (rec.item.path.clone(), rec.cancel.take(), rec.dir.clone())
        };
        // 钩子只做「调度到 UI 线程」,不得同步回调状态表(调用方仍持有全局锁;同步回调会自我等待)
        if let Some(hook) = hook {
            hook();
        }
        let _ = fs::remove_file(&path);
        self.prune_dir(&dir);
        self.mark_dirty();
        Ok(())
    }

    /// 清空已结束记录(进行中保留)
    pub fn clear_finished(&mut self) {
        let before = self.records.len();
        self.records
            .retain(|r| r.item.state == DownloadState::Progress);
        if self.records.len() != before {
            self.mark_dirty();
        }
    }

    /// 删除一条已结束的记录,并尽力删除磁盘文件
    ///
    /// 规则:
    /// - 未知 id → 报错(`download not found: <id>`),不动任何记录;
    /// - 进行中 → 拒绝(先取消,避免删掉正在写入的文件,记录与文件都保留);
    /// - 结束态 → **先移除记录再删文件**;文件不存在/删除失败仍算成功,
    ///   响应 `{deleted:false, filename, error:"<原因>"}`,记录不再出现。
    ///
    /// 返回:响应 `data` 对象(`{deleted, filename, error?}`)
    pub fn delete(&mut self, id: &str) -> Result<Value, String> {
        let Some(pos) = self.records.iter().position(|r| r.item.id == id) else {
            return Err(format!("download not found: {id}"));
        };
        if self.records[pos].item.state == DownloadState::Progress {
            return Err(DELETE_PROGRESS_ERROR.to_string());
        }
        let rec = self.records.remove(pos);
        self.mark_dirty();
        match fs::remove_file(&rec.item.path) {
            Ok(()) => Ok(json!({ "deleted": true, "filename": rec.item.filename })),
            Err(e) => {
                let reason = if e.kind() == std::io::ErrorKind::NotFound {
                    "file not found".to_string()
                } else {
                    e.to_string()
                };
                Ok(json!({
                    "deleted": false,
                    "filename": rec.item.filename,
                    "error": reason,
                }))
            }
        }
    }

    /// 取一条记录的文件名与落盘路径(事件 payload 用;记录可能已被淘汰)
    fn meta(&self, id: &str) -> Option<(String, String)> {
        self.records
            .iter()
            .find(|r| r.item.id == id)
            .map(|r| (r.item.filename.clone(), r.item.path.clone()))
    }

    /// 切换下载目录:校验 → `create_dir_all` → 持久化
    ///
    /// 返回:`{dir, defaultDir, persisted}`;校验/创建失败返回不含路径细节的错误文案
    pub fn set_dir(&mut self, raw: &str) -> Result<Value, String> {
        let path = prepare_dir(Path::new(raw.trim()))?;
        self.dir = path.clone();
        let persisted = self.persist_dir(&path);
        Ok(json!({
            "dir": path.to_string_lossy(),
            "defaultDir": self.default_dir.to_string_lossy(),
            "persisted": persisted,
        }))
    }

    /// 恢复默认目录:删除持久化文件 → 目录切回 [`Self::default_dir`](模板展开结果)
    ///
    /// 返回:`{dir, defaultDir}`;删除持久化文件失败只记日志(内存目录仍切回默认)
    pub fn reset_dir(&mut self) -> Value {
        self.clear_persisted_dir();
        self.dir = self.default_dir.clone();
        let _ = fs::create_dir_all(&self.dir);
        self.dir_value()
    }

    /// 当前目录查询结果
    pub fn dir_value(&self) -> Value {
        json!({
            "dir": self.dir.to_string_lossy(),
            "defaultDir": self.default_dir.to_string_lossy(),
            "limit": self.history_limit,
        })
    }

    /// 列表快照:按目录分组;`limit` 限制每组返回条数(None = 全部已存条目)
    pub fn snapshot(&self, limit: Option<usize>) -> Value {
        let mut groups: Vec<(String, u64, Vec<&Record>)> = Vec::new();
        for rec in &self.records {
            let key = rec.dir.to_string_lossy().to_string();
            match groups.iter_mut().find(|(dir, _, _)| *dir == key) {
                Some((_, activity, items)) => {
                    *activity = (*activity).max(rec.last_activity);
                    items.push(rec);
                }
                None => groups.push((key, rec.last_activity, vec![rec])),
            }
        }
        for (_, _, items) in groups.iter_mut() {
            items.sort_by(|a, b| {
                b.item
                    .started_at
                    .cmp(&a.item.started_at)
                    .then_with(|| b.item.id.cmp(&a.item.id))
            });
            if let Some(n) = limit {
                items.truncate(n);
            }
        }
        groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let groups_json: Vec<Value> = groups
            .iter()
            .map(|(dir, _, items)| {
                let entries: Vec<&DownloadItem> = items.iter().map(|r| &r.item).collect();
                json!({ "dir": dir, "items": entries })
            })
            .collect();
        json!({
            "dir": self.dir.to_string_lossy(),
            "groups": groups_json,
        })
    }

    /// 每组按「最近开始时间」保留 `history_limit` 条结束态记录(进行中不淘汰)
    fn prune_dir(&mut self, dir: &Path) {
        let limit = self.history_limit;
        let mut finished: Vec<usize> = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, r)| r.dir == dir && r.item.state.is_terminal())
            .map(|(i, _)| i)
            .collect();
        if finished.len() <= limit {
            return;
        }
        finished.sort_by_key(|&i| (self.records[i].item.started_at, i));
        let remove_n = finished.len() - limit;
        let mut drop_flags = vec![false; self.records.len()];
        for i in finished.into_iter().take(remove_n) {
            drop_flags[i] = true;
        }
        let mut idx = 0usize;
        self.records.retain(|_| {
            let keep = !drop_flags[idx];
            idx += 1;
            keep
        });
    }

    /// 把目录写入 `<sessionDataDir>/download-dir.txt`;无会话数据目录时返回 false
    fn persist_dir(&self, path: &Path) -> bool {
        let Some(base) = self.session_data_dir.as_ref() else {
            return false;
        };
        if fs::create_dir_all(base).is_err() {
            return false;
        }
        fs::write(base.join(DIR_FILE_NAME), path.to_string_lossy().as_bytes()).is_ok()
    }

    /// 删除 `<sessionDataDir>/download-dir.txt`(文件不存在视为成功)
    fn clear_persisted_dir(&self) {
        let Some(base) = self.session_data_dir.as_ref() else {
            return;
        };
        match fs::remove_file(base.join(DIR_FILE_NAME)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("[downloads] remove persisted dir failed: {e}"),
        }
    }
}

/// 全局记录表
static STATE: OnceLock<Mutex<Downloads>> = OnceLock::new();

/// 访问全局记录表(未初始化时用临时目录兜底,保证端点不 panic)
fn state() -> MutexGuard<'static, Downloads> {
    let mutex = STATE.get_or_init(|| {
        Mutex::new(Downloads::new(
            std::env::temp_dir().join("bt-shell-downloads"),
            None,
            DEFAULT_HISTORY_LIMIT,
        ))
    });
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// 初始化全局记录表(启动时调用一次)
///
/// 启用记录落盘(有 `--user-data-dir` 时)→ 旧路径一次性迁移 → 回填历史 →
/// 启动后台落盘线程。
/// 返回:解析后的下载目录(供启动日志)
pub fn init(cfg: StartupConfig) -> PathBuf {
    let dir = resolve_initial_dir(&cfg);
    let mut downloads =
        Downloads::new(dir.clone(), cfg.session_data_dir.clone(), cfg.history_limit);
    // 默认目录始终按模板 + 占位符展开,不受持久化文件/env/命令行覆盖影响
    downloads.set_default_dir(default_download_dir(&cfg.spec));
    let history = cfg
        .session_data_dir
        .as_deref()
        .map(|base| base.join(HISTORY_FILE_NAME));
    if let Some(path) = history.clone() {
        if let Some(base) = cfg.session_data_dir.as_deref() {
            let legacy = base
                .join(LEGACY_HISTORY_SUBDIR)
                .join(LEGACY_HISTORY_FILE_NAME);
            migrate_legacy_history(&path, &legacy);
        }
        downloads.set_history_path(path);
        downloads.restore();
    }
    if STATE.set(Mutex::new(downloads)).is_ok() {
        if let Some(path) = history {
            eprintln!("[downloads] history={}", path.display());
            spawn_history_writer();
        }
    } else {
        eprintln!("[downloads] already initialized; keeping first configuration");
    }
    dir
}

/// 后台落盘线程:每 [`HISTORY_POLL_MS`] 检查一次,变更静默满 [`HISTORY_DEBOUNCE_MS`] 才写盘
///
/// 写盘与其它写操作共用全局 `Mutex<Downloads>`(临界区只含一次小文件写);
/// 线程随进程退出结束,退出前的最后一次写入由 [`flush_history`] 保证。
fn spawn_history_writer() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(HISTORY_POLL_MS));
            let now = now_ms();
            let mut guard = state();
            guard.flush_if_due(now);
        }
    });
}

/// 退出前强制落盘(不等待防抖窗口;无待写变更时为空操作)
pub fn flush_history() {
    state().flush_history();
}


/// 当前生效目录(供 Tauri 命令/日志)
pub fn current_dir() -> PathBuf {
    state().dir().to_path_buf()
}

/// 每组历史上限
pub fn history_limit() -> usize {
    state().history_limit()
}

/// 采集层:登记下载开始
pub fn begin(url: &str, suggested_path: &str) -> (String, PathBuf) {
    state().begin(url, suggested_path)
}

/// 采集层:登记下载/另存为开始(`kind` = 来源标记,普通下载传 `None`)
pub fn begin_kind(url: &str, suggested_path: &str, kind: Option<&str>) -> (String, PathBuf) {
    state().begin_kind(url, suggested_path, kind)
}

/// 采集层:记录是否仍在进行中(另存为的落盘等待线程据此提前收工)
pub fn is_progress(id: &str) -> bool {
    state().is_progress(id)
}

/// 采集层:注册取消钩子
pub fn set_cancel_hook(id: &str, hook: CancelFn) {
    state().set_cancel_hook(id, hook);
}

/// 采集层:更新进度
pub fn set_progress(id: &str, received: u64, total: u64) {
    state().set_progress(id, received, total);
}

/// 采集层:写入结束状态
///
/// 返回:是否发生了「进行中 → 结束态」的跃迁(采集层据此只广播一次)
pub fn set_state(id: &str, state_value: DownloadState, error: Option<String>) -> bool {
    state().finish(id, state_value, error)
}

/// 面板 webview 是否存在(下载事件只投给面板;未创建时静默跳过)
fn panel_ready(app: &AppHandle) -> bool {
    crate::ui::panel_webview(app).is_some()
}

/// 广播 `downloads-changed`(面板未创建时直接跳过,不报错)
fn emit_changed(app: &AppHandle, payload: Value) {
    if !panel_ready(app) {
        eprintln!("[downloads] skip {CHANGED_EVENT}: panel webview absent");
        return;
    }
    use tauri::Emitter;
    // 广播失败只记日志:下载本身不受影响
    match app.emit(CHANGED_EVENT, payload.clone()) {
        Ok(()) => eprintln!("[downloads] emit {CHANGED_EVENT} {payload}"),
        Err(e) => eprintln!("[downloads] emit {CHANGED_EVENT} failed: {e}"),
    }
}

/// 新下载开始:广播 `{id, filename, state:"progress"}`
///
/// 不改变面板可见性(不打扰用户);面板 webview 未创建时由 `emit_changed` 安全跳过。
pub(crate) fn announce_started(app: &AppHandle, id: &str, filename: &str) {
    emit_changed(
        app,
        download_event_payload(id, Some(filename), DownloadState::Progress),
    );
}

/// 状态变化(完成/失败/取消):广播 `{id, state}`,供前端刷新列表
///
/// 完成态额外带 `filename` / `path`(前端弹「已下载」提示用;另存为也只有这里能拿到最终文件名)。
pub(crate) fn announce_state(app: &AppHandle, id: &str, state_value: DownloadState) {
    let payload = match state_value {
        DownloadState::Completed => match state().meta(id) {
            Some((filename, path)) => download_completed_payload(id, &filename, &path),
            // 记录已被淘汰(极少见):退回不带文件名的形态,前端只是不弹提示
            None => download_event_payload(id, None, state_value),
        },
        _ => download_event_payload(id, None, state_value),
    };
    emit_changed(app, payload);
}

/// `/api/downloads`:`{limit?, clear?, cancel?, delete?}` → 分组快照
///
/// `delete` 是独立动作:只做删除,成功时返回 `data = {deleted, filename, error?}`
/// (不返回快照;面板/模型拿到结果后自行刷新)。
/// 取消成功后会广播 `downloads-changed`(状态 `canceled`),故需要应用句柄。
pub fn api_downloads(app: &AppHandle, body: &Value) -> Result<Value, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = (app, body);
        return Err("downloads is not supported on macOS".to_string());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let limit = parse_limit(body.get("limit"))?;
        let clear = truthy(body.get("clear"))?;
        let cancel = match body.get("cancel") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(_) => return Err("cancel requires a download id".to_string()),
        };
        let delete = match body.get("delete") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
            Some(_) => return Err("delete requires a download id".to_string()),
        };
        // delete 独占:与 clear/cancel 同时给出时只删这一条(不改其它记录)
        if let Some(id) = delete {
            return state().delete(&id);
        }
        let cancelled = {
            let mut guard = state();
            if let Some(id) = cancel {
                // 只有真正从「进行中」切到「已取消」才广播(不存在/非进行中会直接报错)
                guard.cancel(&id)?;
                Some(id)
            } else {
                None
            }
        };
        // 锁已释放再广播:回调可能触发前端立即拉取新快照,避免自我等待
        if let Some(id) = cancelled {
            announce_state(app, &id, DownloadState::Canceled);
        }
        let mut guard = state();
        if clear {
            guard.clear_finished();
        }
        Ok(guard.snapshot(limit))
    }
}

/// `/api/download-dir` 请求体解析结果
#[derive(Debug, PartialEq, Eq)]
enum DirAction {
    /// 无 `set`/`reset`:只查询
    Query,
    /// `{set: "<path>"}`:校验并切换
    Set(String),
    /// `{reset: true}`:删持久化文件并切回默认目录
    Reset,
}

/// 解析 `/api/download-dir` 请求体
///
/// 互斥约定:**同时给 `set` 与 `reset` 时以 `set` 优先**(`reset` 被忽略)。
fn parse_dir_action(body: &Value) -> Result<DirAction, String> {
    match body.get("set") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if !s.trim().is_empty() => {
            return Ok(DirAction::Set(s.trim().to_string()));
        }
        Some(_) => return Err("set must be an absolute path".to_string()),
    }
    if truthy_key(body.get("reset"), "reset")? {
        Ok(DirAction::Reset)
    } else {
        Ok(DirAction::Query)
    }
}

/// `/api/download-dir`:无参数时查询;`set` 切换并持久化;`reset` 删持久化并回默认
///
/// 响应(查询/set/reset 均)包含 `{dir, defaultDir}`;`set` 额外带 `persisted`;
/// 非法路径返回不含路径细节的错误文案。
pub fn api_download_dir(body: &Value) -> Result<Value, String> {
    let action = parse_dir_action(body)?;
    let mut guard = state();
    match action {
        DirAction::Query => Ok(guard.dir_value()),
        DirAction::Set(raw) => guard.set_dir(&raw),
        DirAction::Reset => Ok(guard.reset_dir()),
    }
}

/// 采集层入口:给页面 Webview 挂下载采集(Windows/Linux 真实现;其它平台空实现)
pub fn setup_page(app: &AppHandle, webview: &tauri::webview::Webview) {
    #[cfg(any(windows, target_os = "linux"))]
    {
        platform::setup(app, webview);
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (app, webview);
    }
}

/// 解析启动目录(持久化文件 → env → 命令行 → 默认模板,逐级降级)
fn resolve_initial_dir(cfg: &StartupConfig) -> PathBuf {
    let mut candidates: Vec<(&'static str, PathBuf)> = Vec::new();
    if let Some(base) = cfg.session_data_dir.as_ref() {
        let file = base.join(DIR_FILE_NAME);
        if let Ok(raw) = fs::read_to_string(&file) {
            let raw = raw.trim();
            if !raw.is_empty() {
                candidates.push(("persisted", PathBuf::from(raw)));
            }
        }
    }
    if let Some(raw) = cfg.env_dir.as_ref() {
        candidates.push(("env", expand_template(raw, &cfg.spec)));
    }
    if let Some(raw) = cfg.arg_dir.as_ref() {
        candidates.push(("arg", expand_template(raw, &cfg.spec)));
    }
    candidates.push(("default", expand_template(DEFAULT_DIR_TEMPLATE, &cfg.spec)));
    for (source, path) in candidates {
        match prepare_dir(&path) {
            Ok(path) => {
                eprintln!(
                    "[downloads] dir source={source} path={} limit={}",
                    path.display(),
                    cfg.history_limit
                );
                return path;
            }
            Err(e) => eprintln!("[downloads] ignore {source} download dir: {e}"),
        }
    }
    let fallback = std::env::temp_dir().join("bt-shell-downloads");
    let _ = fs::create_dir_all(&fallback);
    eprintln!(
        "[downloads] dir source=fallback path={}",
        fallback.display()
    );
    fallback
}

/// 校验并创建目录
fn prepare_dir(raw: &Path) -> Result<PathBuf, String> {
    let path = validate_dir(raw)?;
    fs::create_dir_all(&path).map_err(|_| "failed to create download directory".to_string())?;
    Ok(path)
}

/// 目录校验:仅绝对路径;拒绝文件系统根与受保护的系统目录
///
/// 返回:词法归一化后的路径(便于后续持久化/比较);错误文案不含路径细节
pub fn validate_dir(raw: &Path) -> Result<PathBuf, String> {
    if !raw.is_absolute() {
        return Err("download directory must be an absolute path".to_string());
    }
    let path = lexical_normalize(raw);
    if is_filesystem_root(&path) {
        return Err("refusing to use a filesystem root as download directory".to_string());
    }
    if is_protected_dir(&path) {
        return Err("refusing to use a protected system directory".to_string());
    }
    Ok(path)
}

/// 展开目录模板(`<cache>` / `<profile>` / `<sid>` / `<sname>`,后三者做文件名安全化)
pub fn expand_template(template: &str, spec: &DirSpec) -> PathBuf {
    let sname = if spec.sname.trim().is_empty() {
        sanitize_segment(&spec.sid)
    } else {
        sanitize_segment(&spec.sname)
    };
    let out = template
        .replace("<cache>", &spec.cache_root.to_string_lossy())
        .replace("<profile>", &sanitize_segment(&spec.profile))
        .replace("<sid>", &sanitize_segment(&spec.sid))
        .replace("<sname>", &sname);
    PathBuf::from(out)
}

/// 默认下载目录(默认模板 + 占位符展开,不含任何运行时覆盖)
///
/// 做词法归一化:Windows 下把模板里的 `/` 统一成 `\`,使 `defaultDir`
/// 与运行时目录(`validate_dir` 归一化结果)可以用字符串直接比较。
pub fn default_download_dir(spec: &DirSpec) -> PathBuf {
    lexical_normalize(&expand_template(DEFAULT_DIR_TEMPLATE, spec))
}

/// 路径段安全化:只留字母数字与 `-_.`,其它替换为 `_`;去掉首尾 `.`/`_`;空则 `default`
pub fn sanitize_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches(|c| c == '.' || c == '_');
    let limited: String = trimmed.chars().take(MAX_SEGMENT_CHARS).collect();
    if limited.is_empty() {
        DEFAULT_PROFILE.to_string()
    } else {
        limited
    }
}

/// 文件名安全化:去掉路径分隔符/控制字符/Windows 非法字符,保留原扩展名结构
fn sanitize_file_name(raw: &str) -> String {
    // 跨平台取最后一段:Windows 与 Unix 的路径分隔符都要切
    // (Path::file_name 在 Unix 上不认 `\`,会把整个 "C:\dir\a.txt" 当成文件名)
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string();
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let out = out.trim().trim_end_matches('.').trim().to_string();
    let limited: String = out.chars().take(MAX_SEGMENT_CHARS).collect();
    if limited.is_empty() {
        FALLBACK_FILENAME.to_string()
    } else if is_windows_reserved(&limited) {
        format!("_{limited}")
    } else {
        limited
    }
}

/// Windows 保留设备名(直接作文件名会失败)
fn is_windows_reserved(name: &str) -> bool {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem[3..].chars().all(|c| c.is_ascii_digit()))
}

/// 重名时生成 `stem (n).ext`(n 从 1 递增)
pub fn unique_name(filename: &str, mut taken: impl FnMut(&str) -> bool) -> String {
    let safe = sanitize_file_name(filename);
    if !taken(&safe) {
        return safe;
    }
    let stem = Path::new(&safe)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| safe.clone());
    let ext = Path::new(&safe)
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for n in 1..=MAX_NAME_ATTEMPTS {
        let candidate = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("{stem} ({})", now_ms())
}

/// 文件系统根(`/`、`C:\`):有根且没有普通路径段
fn is_filesystem_root(path: &Path) -> bool {
    let mut has_root = false;
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => has_root = true,
            Component::Normal(_) => return false,
            Component::CurDir | Component::ParentDir => return false,
        }
    }
    has_root
}

/// 受保护系统目录(按平台:Windows 系统目录;Linux `/etc`、`/usr` 等;macOS 未采集下载不做额外限制)
fn is_protected_dir(path: &Path) -> bool {
    #[cfg(windows)]
    {
        const ENV_VARS: [&str; 4] = [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramData",
        ];
        let candidate = norm_for_compare(path);
        for var in ENV_VARS {
            if let Ok(base) = std::env::var(var) {
                if is_under(&candidate, &norm_for_compare(Path::new(&base))) {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        const PROTECTED: [&str; 10] = [
            "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys", "/run", "/var",
        ];
        let normalized = path.to_string_lossy().replace('\\', "/");
        if PROTECTED
            .iter()
            .any(|base| normalized == *base || normalized.starts_with(&format!("{base}/")))
        {
            return true;
        }
        // `/lib*`:lib、lib32、lib64、libx32 …
        normalized.strip_prefix("/lib").is_some_and(|rest| {
            rest.is_empty()
                || rest.starts_with('/')
                || rest.starts_with(|c: char| c.is_ascii_digit() || c == 'x')
        })
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        false
    }
}

/// 词法归一化(消掉 `.` / `..`,不访问文件系统)
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // 已到根时 pop 无效,不会越出根
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        out
    }
}

/// 路径比较口径:小写 + 统一分隔符 + 去尾部分隔符
#[cfg(windows)]
fn norm_for_compare(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

/// `candidate` 是否等于 `base` 或位于 `base` 之下(均已按 [`norm_for_compare`] 归一)
fn is_under(candidate: &str, base: &str) -> bool {
    !base.is_empty()
        && (candidate == base
            || (candidate.len() > base.len()
                && candidate.starts_with(base)
                && candidate.as_bytes().get(base.len()) == Some(&b'\\')))
}

/// 从会话数据目录派生 (cache_root, profile, sid)
fn split_session_path(data_dir: Option<&str>) -> (PathBuf, String, String) {
    let Some(dir) = data_dir.map(str::trim).filter(|d| !d.is_empty()) else {
        return (
            home_cache_root(),
            DEFAULT_PROFILE.to_string(),
            "session".to_string(),
        );
    };
    let path = PathBuf::from(dir);
    let sid = path.file_name().map(|n| n.to_string_lossy().to_string());
    let profile = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string());
    let sessions = path.parent().and_then(Path::parent);
    if let (Some(sid), Some(profile), Some(sessions)) = (sid, profile, sessions) {
        if sessions.file_name() == Some(OsStr::new("sessions")) {
            if let Some(cache_root) = sessions.parent() {
                if !cache_root.as_os_str().is_empty() {
                    return (cache_root.to_path_buf(), profile, sid);
                }
            }
        }
    }
    let sid = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "session".to_string());
    (home_cache_root(), DEFAULT_PROFILE.to_string(), sid)
}

/// 家目录下的插件缓存根兜底
fn home_cache_root() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    match home {
        Some(home) => home
            .join(".opencode")
            .join("plugins-cache")
            .join("opencode-browser-tool"),
        None => std::env::temp_dir().join("opencode-browser-tool"),
    }
}

/// 上限参数:接受数字或数字字符串;缺省 = None
fn parse_limit(value: Option<&Value>) -> Result<Option<usize>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|n| Some(n as usize))
            .ok_or_else(|| "limit must be a non-negative integer".to_string()),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => s
            .trim()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| "limit must be a non-negative integer".to_string()),
        Some(_) => Err("limit must be a non-negative integer".to_string()),
    }
}

/// 布尔参数:兼容 JSON bool 与 "true"/"1"/"yes"/"on"
fn truthy(value: Option<&Value>) -> Result<bool, String> {
    truthy_key(value, "clear")
}

/// 布尔参数(错误文案带字段名):兼容 JSON bool 与 "true"/"1"/"yes"/"on"
fn truthy_key(value: Option<&Value>, key: &str) -> Result<bool, String> {
    match value {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::Number(n)) => Ok(n.as_i64().is_some_and(|n| n != 0)),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" | "" => Ok(false),
            _ => Err(format!("{key} must be a boolean")),
        },
        Some(_) => Err(format!("{key} must be a boolean")),
    }
}

/// 当前毫秒时间戳
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 严格单调递增的活动时间(毫秒):同一毫秒内的多次活动也能确定先后顺序
fn tick() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev.saturating_add(1));
        match LAST.compare_exchange_weak(prev, next, Ordering::SeqCst, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => prev = actual,
        }
    }
}

/// 是否该写盘(纯逻辑,便于单测)
///
/// 两个条件任一满足即写:**静默已满 [`HISTORY_DEBOUNCE_MS`]**(合并同一次操作里的多次变更),
/// 或**本轮待写已等满 [`HISTORY_MAX_WAIT_MS`]**(下载进度持续刷新、静默永不到来时兜底)。
///
/// 参数:`now` 当前时间;`changed_at` 最近一次变更时间;`pending_since` 本轮首个变更时间
pub fn history_write_due(now: u64, changed_at: u64, pending_since: u64) -> bool {
    now.saturating_sub(changed_at) >= HISTORY_DEBOUNCE_MS
        || now.saturating_sub(pending_since) >= HISTORY_MAX_WAIT_MS
}

/// 从 `id`(`dl-<序号>`)取序号;形状不符返回 `None`
fn id_seq(id: &str) -> Option<u64> {
    id.strip_prefix("dl-")?.parse::<u64>().ok()
}

/// 状态字符串 → [`DownloadState`](未知值返回 `None`,由调用方决定兜底)
fn state_from_str(raw: &str) -> Option<DownloadState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "progress" => Some(DownloadState::Progress),
        "completed" => Some(DownloadState::Completed),
        "failed" => Some(DownloadState::Failed),
        "canceled" => Some(DownloadState::Canceled),
        _ => None,
    }
}

/// 历史文件里的一条记录 → [`DownloadItem`](缺关键字段/类型不符返回 `None`)
///
/// 宽容解析:数字字段缺失按 0、`state` 无法识别按 `failed`、`filename` 缺失由 `path` 补;
/// `id` 与 `path` 是定位记录的最小信息,缺任一则该条跳过。
fn item_from_json(value: &Value) -> Option<DownloadItem> {
    let id = value.get("id")?.as_str()?.trim().to_string();
    let path = value.get("path")?.as_str()?.trim().to_string();
    if id.is_empty() || path.is_empty() {
        return None;
    }
    let filename = value
        .get("filename")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| FALLBACK_FILENAME.to_string());
    Some(DownloadItem {
        id,
        url: value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        filename,
        path,
        total_bytes: value
            .get("totalBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        received_bytes: value
            .get("receivedBytes")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        state: value
            .get("state")
            .and_then(Value::as_str)
            .and_then(state_from_str)
            .unwrap_or(DownloadState::Failed),
        error: value
            .get("error")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        kind: value
            .get("kind")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        started_at: value.get("startedAt").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// 原子写历史文件:序列化 → [`write_atomic_file`]
///
/// rename 失败时清理临时文件并把错误交给调用方(下次变更重试)。
fn write_history_file(path: &Path, items: &[&DownloadItem]) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(&items).map_err(std::io::Error::other)?;
    write_atomic_file(path, &body)
}

/// 原子写文件:父目录 → 临时文件 → rename 覆盖
///
/// 临时文件带进程号,进程被杀也不会把半截内容留在正式文件上。
/// 参数:`path` 目标文件;`body` 完整内容(调用方保证已是完整快照)
/// 返回:写盘结果;rename 失败时已尽力清理临时文件
fn write_atomic_file(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "history path has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&tmp, body)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// 旧路径 → 新路径一次性迁移(启动时调用)
///
/// 参数:`new_path` 新历史文件(`<sessionDataDir>/download-history.json`);
/// `legacy_path` 旧历史文件(`<sessionDataDir>/downloads/history.json`)
/// 返回:是否真的完成了迁移
///
/// 规则:
/// - 新文件已存在 → 不动(以新文件为准,旧文件保留);
/// - 旧文件不存在 → 不做任何事(正常首启);
/// - 读旧文件失败 / 写新文件失败 → 记日志并保留旧文件(下次启动重试);
/// - 写新成功后删除旧文件,删除失败只记日志(新文件已在,不会重复迁移)。
pub fn migrate_legacy_history(new_path: &Path, legacy_path: &Path) -> bool {
    if new_path.exists() {
        return false;
    }
    if !legacy_path.exists() {
        return false;
    }
    let body = match fs::read(legacy_path) {
        Ok(body) => body,
        Err(e) => {
            eprintln!(
                "[downloads] migrate history: read legacy failed: {e}; keep {}",
                legacy_path.display()
            );
            return false;
        }
    };
    if let Err(e) = write_atomic_file(new_path, &body) {
        eprintln!(
            "[downloads] migrate history: write new failed: {e}; keep {}",
            legacy_path.display()
        );
        return false;
    }
    match fs::remove_file(legacy_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("[downloads] migrate history: remove legacy failed: {e}"),
    }
    eprintln!(
        "[downloads] history migrated: {} → {}",
        legacy_path.display(),
        new_path.display()
    );
    true
}

// ---- 采集实现:Windows(WebView2) / Linux(WebKitGTK) ----

#[cfg(windows)]
mod platform {
    //! WebView2 下载采集
    //!
    //! 线程纪律:WebView2 事件回调在 UI 线程执行,回调里只做取值 + 短临界区写状态,
    //! 不做任何阻塞;取消由 HTTP 线程通过 `run_on_main_thread` 回到 UI 线程执行。
    //! 另存为(`SaveAsUIShowing`)没有进度/完成回调,由**独立后台线程**轮询落盘文件收尾。
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use tauri::AppHandle;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
        COREWEBVIEW2_SAVE_AS_KIND_HTML_ONLY, ICoreWebView2, ICoreWebView2_4, ICoreWebView2_25,
        ICoreWebView2DownloadOperation, ICoreWebView2DownloadStartingEventArgs,
        ICoreWebView2SaveAsUIShowingEventArgs,

    };
    use webview2_com::{
        BytesReceivedChangedEventHandler,
        DownloadStartingEventHandler, SaveAsUIShowingEventHandler, StateChangedEventHandler,
    };
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::core::{HSTRING, Interface, PWSTR};

    use super::{
        DownloadState, begin_kind, is_progress, set_cancel_hook, set_progress, set_state,
    };

    /// 事件 token(持有以防处理器被回收;与进程同寿命)
    static TOKENS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

    /// 待匹配的另存为目标:`(路径比较键, 记录 id, 目标路径)`,上限 [`MAX_PENDING_SAVE_AS`]
    static PENDING_SAVE_AS: Mutex<Vec<(String, String, PathBuf)>> = Mutex::new(Vec::new());

    /// 待匹配列表上限(正常同时最多几条;防止异常路径无限增长)
    const MAX_PENDING_SAVE_AS: usize = 32;
    /// 另存为落盘等待上限:超时仍未出现目标文件 → 记录置 failed
    const SAVE_AS_TIMEOUT: Duration = Duration::from_secs(120);
    /// 落盘轮询间隔;连续两次采样大小一致才算写盘结束
    const SAVE_AS_POLL: Duration = Duration::from_millis(300);

    /// 把 WebView2 返回的 COM 字符串取出并释放(CoTaskMemFree)
    ///
    /// 安全:`ptr` 必须是 WebView2 `get_*` 返回的 CoTaskMemAlloc 字符串或空指针
    unsafe fn take_pwstr(ptr: PWSTR) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let value = unsafe { ptr.to_hstring() }.to_string();
        unsafe {
            CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void));
        }
        value
    }

    /// 读取 WebView2 `get_*` 输出参数(CoTaskMem 字符串);调用失败按空串处理
    ///
    /// 安全:仅用于 WebView2 的 `get_*`(`PWSTR` 出参)接口
    unsafe fn read_string(
        get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>,
    ) -> String {
        let mut raw = PWSTR::null();
        match get(&mut raw) {
            Ok(()) => unsafe { take_pwstr(raw) },
            Err(_) => String::new(),
        }
    }

    /// 路径比较键(Windows 路径大小写不敏感,统一分隔符)
    fn normalize_path_key(raw: &str) -> String {
        raw.trim().replace('/', "\\").to_lowercase()
    }

    /// 登记一条待匹配的另存为目标(供随后的 `DownloadStarting` 复用,避免重复记录)
    fn remember_pending(path: &Path, id: &str) {
        let mut pending = PENDING_SAVE_AS.lock().unwrap();
        if pending.len() >= MAX_PENDING_SAVE_AS {
            pending.remove(0);
        }
        pending.push((
            normalize_path_key(&path.to_string_lossy()),
            id.to_string(),
            path.to_path_buf(),
        ));
    }

    /// 取出与 `DownloadStarting` 建议路径匹配的另存为记录(命中即移除)
    fn take_pending(suggested: &str) -> Option<(String, PathBuf)> {
        let key = normalize_path_key(suggested);
        let mut pending = PENDING_SAVE_AS.lock().unwrap();
        let index = pending.iter().position(|(k, _, _)| *k == key)?;
        let (_, id, path) = pending.remove(index);
        Some((id, path))
    }

    /// 给页面 Webview 注册 DownloadStarting + SaveAsUIShowing(每个 webview 独立)
    pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
        let handle = app.clone();
        let save_as_handle = app.clone();
        let _ = webview.with_webview(move |platform_webview| unsafe {
            let controller = platform_webview.controller();
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            // 另存为事件(ICoreWebView2_25)与普通下载独立;运行时不支持时只记日志
            setup_save_as(&save_as_handle, &core);
            let Ok(wv4) = core.cast::<ICoreWebView2_4>() else {
                return;
            };
            let handler = DownloadStartingEventHandler::create(Box::new(
                move |_sender: Option<ICoreWebView2>,
                      args: Option<ICoreWebView2DownloadStartingEventArgs>|
                      -> windows::core::Result<()> {
                    if let Some(args) = args {
                        on_starting(&handle, &args);
                    }
                    Ok(())
                },
            ));
            let mut token = 0i64;
            if wv4.add_DownloadStarting(&handler, &mut token).is_ok() {
                TOKENS.lock().unwrap().push(token);
            }
        });
    }

    /// 下载开始:改保存路径到当前目录,并订阅进度/状态
    ///
    /// 若这次下载来自已被改为静默落盘的另存为,`ResultFilePath` 就是那条记录已选好的路径:
    /// 复用它(不新建记录、不改路径),避免同一文件出现两条记录。
    fn on_starting(app: &AppHandle, args: &ICoreWebView2DownloadStartingEventArgs) {
        unsafe {
            let Ok(op) = args.DownloadOperation() else {
                return;
            };
            let suggested = read_string(|out| op.ResultFilePath(out));
            let url = read_string(|out| op.Uri(out));
            let reused = take_pending(&suggested);
            let fresh = reused.is_none();
            let (id, path) = match reused {
                Some(found) => found,
                None => super::begin(&url, &suggested),
            };
            if fresh {
                let target = HSTRING::from(path.to_string_lossy().as_ref());
                let _ = args.SetResultFilePath(&target);
            }
            let _ = args.SetHandled(true);
            // 首帧状态(下载可能在订阅前就已推进)
            sync_progress(&id, &op);
            sync_state(app, &id, &op);
            let bytes_id = id.clone();
            let bytes_handler = BytesReceivedChangedEventHandler::create(Box::new(
                move |op: Option<ICoreWebView2DownloadOperation>,
                      _: Option<windows::core::IUnknown>| {
                    if let Some(op) = op {
                        sync_progress(&bytes_id, &op);
                    }
                    Ok(())
                },
            ));
            let mut bytes_token = 0i64;
            if op
                .add_BytesReceivedChanged(&bytes_handler, &mut bytes_token)
                .is_ok()
            {
                TOKENS.lock().unwrap().push(bytes_token);
            }
            let state_id = id.clone();
            let state_app = app.clone();
            let state_handler = StateChangedEventHandler::create(Box::new(
                move |op: Option<ICoreWebView2DownloadOperation>,
                      _: Option<windows::core::IUnknown>| {
                    if let Some(op) = op {
                        sync_state(&state_app, &state_id, &op);
                    }
                    Ok(())
                },
            ));
            let mut state_token = 0i64;
            if op
                .add_StateChanged(&state_handler, &mut state_token)
                .is_ok()
            {
                TOKENS.lock().unwrap().push(state_token);
            }
            // 取消钩子:回到 UI 线程调用(COM 对象不跨线程使用)
            let app_for_cancel = app.clone();
            let op_for_cancel = SendOp(op);
            set_cancel_hook(
                &id,
                Box::new(move || {
                    let _ = app_for_cancel.run_on_main_thread(move || op_for_cancel.cancel());
                }),
            );
            // 新下载开始:确保面板可见并通知前端切到下载视图(必须在订阅之后,列表已含该记录)
            // 另存为复用的记录在 SaveAsUIShowing 时已广播过,这里不重复发
            if fresh {
                let filename = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                super::announce_started(app, &id, &filename);
            }
        }
    }

    /// 注册另存为事件(WebView2 Runtime ≥ 1.0.2739.15 才有 `ICoreWebView2_25`)
    ///
    /// 取不到接口时只记日志并保持系统对话框,不影响 `DownloadStarting` 等其它采集。
    fn setup_save_as(app: &AppHandle, core: &ICoreWebView2) {
        let Ok(wv25) = core.cast::<ICoreWebView2_25>() else {
            eprintln!(
                "[downloads] save-as: ICoreWebView2_25 unavailable (WebView2 runtime too old); system dialog kept"
            );
            return;
        };
        let handle = app.clone();
        let handler = SaveAsUIShowingEventHandler::create(Box::new(
            move |sender: Option<ICoreWebView2>,
                  args: Option<ICoreWebView2SaveAsUIShowingEventArgs>|
                  -> windows::core::Result<()> {
                if let Some(args) = args {
                    on_save_as_showing(&handle, sender.as_ref(), &args);
                }
                Ok(())
            },
        ));
        let mut token = 0i64;
        match unsafe { wv25.add_SaveAsUIShowing(&handler, &mut token) } {
            Ok(()) => {
                TOKENS.lock().unwrap().push(token);
                eprintln!("[downloads] save-as: hooked (silent save into download dir)");
            }
            Err(e) => eprintln!("[downloads] save-as: hook failed: {e}"),
        }
    }

    /// 另存为:压掉系统对话框、改到当前下载目录(同 `unique_name` 去重),并登记记录
    fn on_save_as_showing(
        app: &AppHandle,
        sender: Option<&ICoreWebView2>,
        args: &ICoreWebView2SaveAsUIShowingEventArgs,
    ) {
        unsafe {
            // 系统建议路径(只取文件名);事件本身不给资源 URL,用触发页地址代替(列表按 host 展示)
            let suggested = read_string(|out| args.SaveAsFilePath(out));
            let mime = read_string(|out| args.ContentMimeType(out));
            let url = sender
                .map(|view| read_string(|out| view.Source(out)))
                .unwrap_or_default();
            // 来源判定:HTML 内容(整页另存)= save-page,其余(图片/链接/媒体)= save-as
            let kind = super::classify_save_kind(&mime, &suggested);
            let page = kind == super::SAVE_PAGE_KIND;
            let (id, path) = begin_kind(&url, &suggested, Some(kind));
            let target = HSTRING::from(path.to_string_lossy().as_ref());
            // 顺序:先改路径,再压对话框(SuppressDefaultDialog=TRUE 时才采用 SaveAsFilePath)
            let set_path = args.SetSaveAsFilePath(&target);
            let suppress = args.SetSuppressDefaultDialog(true);
            // 唯一名已保证同名不存在;显式禁止覆盖,避免极端竞态下踩掉别人的文件
            let no_replace = args.SetAllowReplace(false);
            // HTML 文档只接受 HTML_ONLY / SINGLE_FILE / COMPLETE(Kind 缺省值 DEFAULT 不被接受),
            // 压制对话框后无法让用户选格式 → 取 HTML_ONLY(单文件,落盘时机可确定)
            let set_kind = if page {
                args.SetKind(COREWEBVIEW2_SAVE_AS_KIND_HTML_ONLY)
            } else {
                Ok(())
            };
            if set_path.is_err() || suppress.is_err() || no_replace.is_err() || set_kind.is_err() {
                eprintln!(
                    "[downloads] save-as: args rejected kind={kind} path={}",
                    path.display()
                );
                if set_state(
                    &id,
                    DownloadState::Failed,
                    Some("save-as request rejected".to_string()),
                ) {
                    super::announce_state(app, &id, DownloadState::Failed);
                }
                return;
            }
            remember_pending(&path, &id);
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            eprintln!(
                "[downloads] save-as: id={id} kind={kind} mime={mime} file={}",
                path.display()
            );
            super::announce_started(app, &id, &filename);
            watch_save_as(app.clone(), id, path);
        }
    }

    /// 等目标文件落盘并收尾(另存为没有进度/完成回调)
    ///
    /// 引擎先写临时文件、写完才改名到目标路径:目标出现且大小连续两次采样一致 → completed;
    /// 超时(或记录已被 `DownloadStarting`/取消置为终态)→ failed / 直接退出。
    fn watch_save_as(app: AppHandle, id: String, path: PathBuf) {
        std::thread::spawn(move || {
            let deadline = Instant::now() + SAVE_AS_TIMEOUT;
            let mut last_size: Option<u64> = None;
            loop {
                if !is_progress(&id) {
                    // 已被 DownloadStarting 的状态回调或取消收尾,无需再等
                    return;
                }
                let size = std::fs::metadata(&path)
                    .map(|meta| meta.len())
                    .ok()
                    .filter(|len| *len > 0);
                if size.is_some() && size == last_size {
                    if set_state(&id, DownloadState::Completed, None) {
                        super::announce_state(&app, &id, DownloadState::Completed);
                    }
                    eprintln!("[downloads] save-as done: {id}");
                    return;
                }
                last_size = size;
                if Instant::now() >= deadline {
                    if set_state(
                        &id,
                        DownloadState::Failed,
                        Some("save-as timed out".to_string()),
                    ) {
                        super::announce_state(&app, &id, DownloadState::Failed);
                    }
                    eprintln!("[downloads] save-as timeout: {id}");
                    return;
                }
                std::thread::sleep(SAVE_AS_POLL);
            }
        });
    }

    /// 读取并写入当前进度
    fn sync_progress(id: &str, op: &ICoreWebView2DownloadOperation) {
        unsafe {
            let mut received = 0i64;
            let mut total = 0i64;
            let _ = op.BytesReceived(&mut received);
            let _ = op.TotalBytesToReceive(&mut total);
            set_progress(id, received.max(0) as u64, total.max(0) as u64);
        }
    }

    /// 读取并写入当前状态(进行中不写;完成/中断才落终态)
    ///
    /// 只有真正发生跃迁时才广播一次 `downloads-changed`(重复回调静默丢弃)
    fn sync_state(app: &AppHandle, id: &str, op: &ICoreWebView2DownloadOperation) {
        unsafe {
            let mut state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
            if op.State(&mut state).is_err() {
                return;
            }
            if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
                sync_progress(id, op);
                if set_state(id, DownloadState::Completed, None) {
                    super::announce_state(app, id, DownloadState::Completed);
                }
            } else if state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
                let error = interrupt_reason_text(op);
                if set_state(id, DownloadState::Failed, Some(error)) {
                    super::announce_state(app, id, DownloadState::Failed);
                }
            }
        }
    }

    /// 中断原因 → 稳定文案(不含路径)
    fn interrupt_reason_text(op: &ICoreWebView2DownloadOperation) -> String {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON;
        unsafe {
            let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON(0);
            if op.InterruptReason(&mut reason).is_err() {
                return "download interrupted".to_string();
            }
            format!("download interrupted (reason {})", reason.0)
        }
    }

    /// WebView2 COM 接口本身不是 `Send`;这里只在 `run_on_main_thread` 闭包里解包使用,
    /// 跨线程搬运期间不对其调用任何 COM 方法(移动/析构都在 UI 线程完成)。
    struct SendOp(ICoreWebView2DownloadOperation);
    unsafe impl Send for SendOp {}

    impl SendOp {
        /// 在 UI 线程取消下载(调用方保证在主线程执行)
        unsafe fn cancel(self) {
            unsafe {
                let _ = self.0.Cancel();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{normalize_path_key, remember_pending, take_pending};
        use std::path::PathBuf;

        /// 大小写/分隔符不同的同一路径也必须命中;命中后即从待匹配表移除
        #[test]
        fn pending_save_as_matches_loosely_and_takes_once() {
            let path = PathBuf::from(r"C:\tmp\bt-saveas-test\pic.png");
            remember_pending(&path, "dl-saveas-1");
            let hit = take_pending(r"c:/TMP/BT-saveas-test/PIC.PNG");
            assert_eq!(hit.map(|(id, _)| id).as_deref(), Some("dl-saveas-1"));
            assert!(take_pending(r"C:\tmp\bt-saveas-test\pic.png").is_none());
        }

        /// 路径比较键:统一分隔符 + 小写(non-Windows 路径也按此规则比较)
        #[test]
        fn normalize_path_key_unifies_separators_and_case() {
            assert_eq!(
                normalize_path_key(r"C:\A\b.png"),
                normalize_path_key("c:/a/B.PNG")
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    //! WebKitGTK 下载采集
    //!
    //! `download-started` 挂在 WebContext 上(所有页面 webview 共用默认 context),
    //! 因此需要去重注册;真正的保存路径在 `decide-destination` 里决定。
    use std::sync::Mutex;

    use tauri::AppHandle;
    use webkit2gtk::{DownloadExt, URIRequestExt, URIResponseExt, WebContextExt, WebViewExt};

    use super::{DownloadState, set_cancel_hook, set_progress, set_state};

    /// 已注册的 WebContext 指针(去重:每个 context 只挂一次)
    static REGISTERED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    /// 绑定到主线程的下载对象指针(仅用于 cancel;见 [`cancel_on_main`])
    struct SendPtr(usize);
    unsafe impl Send for SendPtr {}

    /// 给页面 Webview 所在 WebContext 注册 download-started
    pub fn setup(app: &AppHandle, webview: &tauri::webview::Webview) {
        let app = app.clone();
        let _ = webview.with_webview(move |platform_webview| {
            let view = platform_webview.inner();
            let Some(context) = view.context() else {
                return;
            };
            let ptr = {
                use gtk::glib::object::ObjectType;
                context.as_ptr() as usize
            };
            {
                let mut registered = REGISTERED.lock().unwrap();
                if registered.contains(&ptr) {
                    return;
                }
                registered.push(ptr);
            }
            let app = app.clone();
            context.connect_download_started(move |_context, download| {
                let url = download
                    .request()
                    .and_then(|req| req.uri())
                    .map(|uri| uri.to_string())
                    .unwrap_or_default();
                let total = download
                    .response()
                    .map(|resp| resp.content_length())
                    .unwrap_or(0);
                let app_decide = app.clone();
                download.connect_decide_destination(move |download, suggested| {
                    let (id, path) = super::begin(&url, suggested);
                    download.set_destination(&path.to_string_lossy());
                    let progress_id = id.clone();
                    download.connect_received_data(move |download, _len| {
                        set_progress(&progress_id, download.received_data_length(), total);
                    });
                    let finish_id = id.clone();
                    let app_finish = app_decide.clone();
                    download.connect_finished(move |_download| {
                        if set_state(&finish_id, DownloadState::Completed, None) {
                            super::announce_state(&app_finish, &finish_id, DownloadState::Completed);
                        }
                    });
                    let fail_id = id.clone();
                    let app_fail = app_decide.clone();
                    download.connect_failed(move |_download, error| {
                        if set_state(
                            &fail_id,
                            DownloadState::Failed,
                            Some(error.message().to_string()),
                        ) {
                            super::announce_state(&app_fail, &fail_id, DownloadState::Failed);
                        }
                    });
                    // 取消:WebKitDownload 只能在 GTK 主线程调用,这里把指针送回主循环
                    let raw = SendPtr({
                        use gtk::glib::object::ObjectType;
                        download.as_ptr() as usize
                    });
                    set_cancel_hook(
                        &id,
                        Box::new(move || {
                            let SendPtr(ptr) = raw;
                            gtk::glib::idle_add_once(move || unsafe {
                                use gtk::glib::translate::from_glib_none;
                                let download: webkit2gtk::Download =
                                    from_glib_none(ptr as *mut webkit2gtk::ffi::WebKitDownload);
                                download.cancel();
                            });
                        }),
                    );
                    // 新下载开始:确保面板可见并通知前端切到下载视图
                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    super::announce_started(&app_decide, &id, &filename);
                    true
                });
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 建一个独立临时目录(调用方负责清理)
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bt-downloads-test-{}-{}-{}",
            tag,
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// 快照里的记录总数(跨分组)
    fn record_count(downloads: &Downloads) -> usize {
        let snapshot = downloads.snapshot(None);
        snapshot["groups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|g| g["items"].as_array().map_or(0, |items| items.len()))
                    .sum()
            })
            .unwrap_or(0)
    }

    /// 模板展开:`<profile>`/`<sid>`/`<sname>` 都做安全化
    #[test]
    fn template_expansion_sanitizes_segments() {
        let spec = DirSpec {
            cache_root: PathBuf::from("/cache"),
            profile: "p/1".into(),
            sid: "../s\\2".into(),
            sname: "a b:c".into(),
        };
        let path = expand_template(&format!("{DEFAULT_DIR_TEMPLATE}/<sname>"), &spec);
        let text = path.to_string_lossy().replace('\\', "/");
        assert!(
            text.ends_with("/cache/sessions/p_1/s_2/downloads/a_b_c"),
            "got {text}"
        );
    }

    /// `<sname>` 为空时回退到 `<sid>`;空段兜底为 `default`
    #[test]
    fn template_expansion_sname_falls_back_to_sid() {
        let spec = DirSpec {
            cache_root: PathBuf::from("/cache"),
            profile: String::new(),
            sid: "sid-9".into(),
            sname: "  ".into(),
        };
        let path = expand_template("<cache>/<profile>/<sname>", &spec);
        let text = path.to_string_lossy().replace('\\', "/");
        assert_eq!(text, "/cache/default/sid-9");
    }

    /// 目录校验:相对路径、文件系统根、系统目录黑名单
    #[test]
    fn dir_validation_rejects_unsafe_paths() {
        assert!(validate_dir(Path::new("relative/downloads")).is_err());
        assert!(validate_dir(Path::new("./downloads")).is_err());

        #[cfg(windows)]
        {
            for bad in ["C:\\", "D:\\"] {
                assert!(validate_dir(Path::new(bad)).is_err(), "must reject {bad}");
            }
            for var in ["SystemRoot", "ProgramFiles", "ProgramData"] {
                if let Ok(base) = std::env::var(var) {
                    assert!(
                        validate_dir(Path::new(&base)).is_err(),
                        "must reject {var}={base}"
                    );
                    let nested = Path::new(&base).join("bt-shell-test");
                    assert!(
                        validate_dir(&nested).is_err(),
                        "must reject nested under {var}"
                    );
                }
            }
            // 词法归一化后仍落在系统目录 → 拒绝
            if let Ok(base) = std::env::var("SystemRoot") {
                let tricky = format!("{}\\temp\\..\\", base);
                assert!(validate_dir(Path::new(&tricky)).is_err());
            }
            let ok = std::env::temp_dir();
            assert!(validate_dir(&ok).is_ok(), "temp dir should be allowed");
        }

        #[cfg(target_os = "linux")]
        {
            for bad in [
                "/", "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/proc", "/sys",
            ] {
                assert!(validate_dir(Path::new(bad)).is_err(), "must reject {bad}");
            }
            for bad in ["/lib", "/lib64", "/lib32", "/etc/cron.d"] {
                assert!(validate_dir(Path::new(bad)).is_err(), "must reject {bad}");
            }
            assert!(validate_dir(Path::new("/tmp/bt-downloads")).is_ok());
        }
    }

    /// 受保护目录前缀匹配:不做「同名前缀」误判(仅 Windows 比较口径)
    #[cfg(windows)]
    #[test]
    fn protected_prefix_matching_is_exact_on_segment_boundary() {
        assert!(is_under("c:\\windows\\system32", "c:\\windows"));
        assert!(is_under("c:\\windows", "c:\\windows"));
        assert!(!is_under("c:\\windowsx", "c:\\windows"));
        assert!(!is_under("c:\\windows", ""));
    }

    /// 重名自动加 ` (n)` 后缀(含无扩展名)
    #[test]
    fn duplicate_names_get_numbered_suffix() {
        assert_eq!(unique_name("a.txt", |_| false), "a.txt");
        assert_eq!(unique_name("a.txt", |n| n == "a.txt"), "a (1).txt");
        assert_eq!(
            unique_name("a.txt", |n| n == "a.txt" || n == "a (1).txt"),
            "a (2).txt"
        );
        assert_eq!(unique_name("noext", |n| n == "noext"), "noext (1)");
        assert_eq!(unique_name("", |_| false), FALLBACK_FILENAME);
        assert_eq!(unique_name("..\\evil:name.exe", |_| false), "evil_name.exe");
    }

    /// begin():磁盘已存在同名文件时实际落到 ` (1)` 后缀
    #[test]
    fn begin_avoids_existing_file() {
        let dir = temp_dir("begin");
        fs::write(dir.join("a.txt"), b"old").unwrap();
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/a.txt", "C:\\suggested\\a.txt");
        assert_eq!(path.file_name().unwrap(), OsStr::new("a (1).txt"));
        assert_eq!(path.parent().unwrap(), dir.as_path());
        let snapshot = downloads.snapshot(None);
        assert_eq!(snapshot["groups"][0]["items"][0]["id"], json!(id));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 另存为记录带 `kind`;普通下载不带该字段(snapshot 里不出现)
    #[test]
    fn save_as_records_carry_kind() {
        let dir = temp_dir("kind");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (save_id, save_path) = downloads.begin_kind(
            "http://example.com/page",
            "C:\\suggested\\pic.png",
            Some(SAVE_AS_KIND),
        );
        assert_eq!(save_path.parent().unwrap(), dir.as_path());
        assert!(downloads.is_progress(&save_id));
        let (plain_id, _) = downloads.begin("http://example.com/a.bin", "a.bin");
        let snapshot = downloads.snapshot(None);
        let items = snapshot["groups"][0]["items"].as_array().cloned().unwrap();
        let find = |id: &str| {
            items
                .iter()
                .find(|item| item["id"] == json!(id))
                .cloned()
                .expect("record present")
        };
        assert_eq!(find(&save_id)["kind"], json!(SAVE_AS_KIND));
        assert!(
            find(&plain_id).get("kind").is_none(),
            "普通下载不应带 kind: {:?}",
            find(&plain_id)
        );
        // 终态之后 is_progress 归 false(落盘等待线程据此收工)
        downloads.finish(&save_id, DownloadState::Completed, None);
        assert!(!downloads.is_progress(&save_id));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 每组上限:70 条完成记录只留最近 64 条;进行中不被淘汰
    #[test]
    fn history_limit_keeps_newest_and_never_evicts_in_progress() {
        let dir = temp_dir("prune");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        for i in 0..70 {
            let (id, _) = downloads.begin(&format!("http://example.com/{i}"), "f.bin");
            downloads.set_progress(&id, 10, 10);
            downloads.finish(&id, DownloadState::Completed, None);
        }
        let snapshot = downloads.snapshot(None);
        let items = snapshot["groups"][0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 64, "每组上限 64");
        assert_eq!(items[0]["id"], json!("dl-69"), "最新的排最前");
        assert!(
            !items.iter().any(|it| it["id"] == json!("dl-0")),
            "最旧的被淘汰"
        );

        // 进行中的记录不参与淘汰:再起 3 条进行中,仍有 64 + 3 条
        for i in 0..3 {
            downloads.begin(&format!("http://example.com/live/{i}"), "live.bin");
        }
        let snapshot = downloads.snapshot(None);
        let items = snapshot["groups"][0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 67);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 分组:不同目录各成一组,组按最近活动降序
    #[test]
    fn snapshot_groups_by_directory_most_recent_first() {
        let dir_a = temp_dir("group-a");
        let dir_b = temp_dir("group-b");
        let mut downloads = Downloads::new(dir_a.clone(), None, 64);
        let (id_a, _) = downloads.begin("http://a/1", "a.bin");
        downloads.finish(&id_a, DownloadState::Completed, None);
        downloads.set_dir(&dir_b.to_string_lossy()).unwrap();
        let (id_b, _) = downloads.begin("http://b/2", "b.bin");
        downloads.finish(&id_b, DownloadState::Completed, None);

        let snapshot = downloads.snapshot(None);
        let groups = snapshot["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups[0]["dir"],
            json!(dir_b.to_string_lossy()),
            "最近活动组在前"
        );
        assert_eq!(groups[1]["dir"], json!(dir_a.to_string_lossy()));
        assert_eq!(snapshot["dir"], json!(dir_b.to_string_lossy()));
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    /// 取消:标记 canceled + 调用钩子 + 删除半成品;重复取消报错
    #[test]
    fn cancel_marks_state_calls_hook_and_removes_partial() {
        let dir = temp_dir("cancel");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/big.bin", "big.bin");
        fs::write(&path, b"partial-bytes").unwrap();
        let hook_called = Arc::new(AtomicBool::new(false));
        let flag = hook_called.clone();
        downloads.set_cancel_hook(
            &id,
            Box::new(move || {
                flag.store(true, Ordering::SeqCst);
            }),
        );
        downloads.cancel(&id).unwrap();
        assert!(hook_called.load(Ordering::SeqCst), "取消钩子必须被调用");
        assert!(!path.exists(), "半成品文件应被删除");
        let snapshot = downloads.snapshot(None);
        assert_eq!(
            snapshot["groups"][0]["items"][0]["state"],
            json!("canceled")
        );
        assert!(downloads.cancel(&id).is_err(), "非进行中不可再取消");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 结束状态不会被后续回调覆盖;clear 只清结束态
    #[test]
    fn terminal_state_is_sticky_and_clear_keeps_in_progress() {
        let dir = temp_dir("sticky");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, _) = downloads.begin("http://example.com/1", "1.bin");
        downloads.finish(&id, DownloadState::Completed, None);
        downloads.finish(&id, DownloadState::Failed, Some("late".into()));
        let snapshot = downloads.snapshot(None);
        assert_eq!(
            snapshot["groups"][0]["items"][0]["state"],
            json!("completed")
        );

        let (live, _) = downloads.begin("http://example.com/2", "2.bin");
        downloads.clear_finished();
        let snapshot = downloads.snapshot(None);
        let items = snapshot["groups"][0]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], json!(live));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 目录持久化:set 写 `download-dir.txt`,重启解析以文件为准
    #[test]
    fn set_dir_persists_and_wins_on_restart() {
        let data_dir = temp_dir("session-data");
        let target = temp_dir("persist-target");
        let mut downloads = Downloads::new(data_dir.join("downloads"), Some(data_dir.clone()), 64);
        let value = downloads.set_dir(&target.to_string_lossy()).unwrap();
        assert_eq!(value["persisted"], json!(true));
        let written = fs::read_to_string(data_dir.join(DIR_FILE_NAME)).unwrap();
        assert_eq!(written, target.to_string_lossy());

        let cfg = StartupConfig {
            spec: DirSpec::default(),
            session_data_dir: Some(data_dir.clone()),
            history_limit: 64,
            env_dir: None,
            arg_dir: Some(
                std::env::temp_dir()
                    .join("ignored-arg")
                    .to_string_lossy()
                    .to_string(),
            ),
        };
        assert_eq!(resolve_initial_dir(&cfg), target);
        let _ = fs::remove_dir_all(&data_dir);
        let _ = fs::remove_dir_all(&target);
    }

    /// `defaultDir` = 默认模板 + 占位符展开,不受运行时覆盖影响
    #[test]
    fn default_dir_expands_template_without_runtime_overrides() {
        let spec = DirSpec {
            cache_root: PathBuf::from("/cache"),
            profile: "work".into(),
            sid: "sid-1".into(),
            sname: "会话 A".into(),
        };
        let text = default_download_dir(&spec)
            .to_string_lossy()
            .replace('\\', "/");
        assert_eq!(text, "/cache/sessions/work/sid-1/downloads");
    }

    /// 查询与 set 响应都带 `defaultDir`
    #[test]
    fn dir_value_includes_default_dir() {
        let data_dir = temp_dir("dir-value-data");
        let default = temp_dir("dir-value-default");
        let target = temp_dir("dir-value-target");
        let mut downloads = Downloads::new(data_dir.join("downloads"), Some(data_dir.clone()), 64);
        downloads.set_default_dir(default.clone());
        let value = downloads.dir_value();
        assert_eq!(
            value["dir"],
            json!(data_dir.join("downloads").to_string_lossy())
        );
        assert_eq!(value["defaultDir"], json!(default.to_string_lossy()));
        let set = downloads.set_dir(&target.to_string_lossy()).unwrap();
        assert_eq!(set["dir"], json!(target.to_string_lossy()));
        assert_eq!(set["defaultDir"], json!(default.to_string_lossy()));
        for dir in [&data_dir, &default, &target] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// reset:删除 `download-dir.txt` 并切回默认目录;响应 `dir == defaultDir`
    #[test]
    fn reset_restores_default_and_removes_persisted_file() {
        let data_dir = temp_dir("reset-data");
        let default = temp_dir("reset-default");
        let target = temp_dir("reset-target");
        let mut downloads = Downloads::new(default.clone(), Some(data_dir.clone()), 64);
        downloads.set_default_dir(default.clone());
        let set = downloads.set_dir(&target.to_string_lossy()).unwrap();
        assert_eq!(set["dir"], json!(target.to_string_lossy()));
        let file = data_dir.join(DIR_FILE_NAME);
        assert!(file.exists(), "set 应写持久化文件");

        let value = downloads.reset_dir();
        assert_eq!(value["dir"], json!(default.to_string_lossy()));
        assert_eq!(value["dir"], value["defaultDir"], "reset 后 dir 应等于默认目录");
        assert!(!file.exists(), "reset 应删除持久化文件");
        assert_eq!(downloads.dir(), default.as_path());
        for dir in [&data_dir, &default, &target] {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// `set` 与 `reset` 同时给时以 `set` 优先;空/非法 `set` 仍报错
    #[test]
    fn dir_action_prefers_set_over_reset() {
        assert_eq!(parse_dir_action(&json!({})).unwrap(), DirAction::Query);
        assert_eq!(
            parse_dir_action(&json!({ "reset": true })).unwrap(),
            DirAction::Reset
        );
        assert_eq!(
            parse_dir_action(&json!({ "reset": "true" })).unwrap(),
            DirAction::Reset
        );
        assert_eq!(
            parse_dir_action(&json!({ "set": " D:\\tmp\\dl ", "reset": true })).unwrap(),
            DirAction::Set("D:\\tmp\\dl".to_string())
        );
        assert_eq!(
            parse_dir_action(&json!({ "set": "D:\\tmp\\dl", "reset": false })).unwrap(),
            DirAction::Set("D:\\tmp\\dl".to_string())
        );
        assert!(parse_dir_action(&json!({ "set": "" })).is_err());
        assert!(parse_dir_action(&json!({ "set": 1 })).is_err());
        assert!(parse_dir_action(&json!({ "reset": "maybe" })).is_err());
    }

    /// `--user-data-dir` 派生占位符:`<cache>/sessions/<profile>/<sid>`
    #[test]
    fn session_path_derives_template_values() {
        let dir = Path::new("/cache/root/sessions/work/sid-1");
        let spec = DirSpec::for_data_dir(Some(&dir.to_string_lossy()), "会话 A");
        assert_eq!(spec.cache_root, PathBuf::from("/cache/root"));
        assert_eq!(spec.profile, "work");
        assert_eq!(spec.sid, "sid-1");
        assert_eq!(sanitize_segment(&spec.sname), "A");
        // 默认模板落到会话目录下
        let expanded = expand_template(DEFAULT_DIR_TEMPLATE, &spec)
            .to_string_lossy()
            .replace('\\', "/");
        assert!(expanded.ends_with("/cache/root/sessions/work/sid-1/downloads"));
    }

    /// 端点参数:limit 兼容数字/字符串,clear/cancel 非法值明确报错
    #[test]
    fn api_arguments_are_validated() {
        assert_eq!(parse_limit(None).unwrap(), None);
        assert_eq!(parse_limit(Some(&json!(5))).unwrap(), Some(5));
        assert_eq!(parse_limit(Some(&json!("7"))).unwrap(), Some(7));
        assert!(parse_limit(Some(&json!(-1))).is_err());
        assert!(parse_limit(Some(&json!("x"))).is_err());
        assert!(truthy(Some(&json!(true))).unwrap());
        assert!(truthy(Some(&json!("1"))).unwrap());
        assert!(!truthy(Some(&json!("false"))).unwrap());
        assert!(truthy(Some(&json!("maybe"))).is_err());
    }

    /// 开始事件 payload:带 id / 最终文件名 / 固定 progress 状态
    #[test]
    fn start_event_payload_carries_id_filename_and_progress_state() {
        let payload = download_event_payload(
            "dl-3",
            Some("报告 (1).pdf"),
            DownloadState::Progress,
        );
        assert_eq!(
            payload,
            json!({ "id": "dl-3", "filename": "报告 (1).pdf", "state": "progress" })
        );
        // 序列化后字段名/状态串必须与前端类型一致(小写)
        assert_eq!(
            payload.to_string(),
            r#"{"filename":"报告 (1).pdf","id":"dl-3","state":"progress"}"#
        );
    }

    /// 状态事件 payload:只带 id + 结束态(completed/failed/canceled),不带文件名
    #[test]
    fn state_event_payload_covers_three_terminal_states_without_filename() {
        for (state, expected) in [
            (DownloadState::Completed, "completed"),
            (DownloadState::Failed, "failed"),
            (DownloadState::Canceled, "canceled"),
        ] {
            let payload = download_event_payload("dl-7", None, state);
            assert_eq!(payload, json!({ "id": "dl-7", "state": expected }));
            assert!(payload.get("filename").is_none(), "状态事件不带文件名");
            assert_eq!(
                payload.to_string(),
                format!(r#"{{"id":"dl-7","state":"{expected}"}}"#)
            );
        }
    }

    /// 广播判定:进度更新不广播;只有「进行中 → 结束态」那一次广播
    #[test]
    fn emit_only_on_start_or_progress_to_terminal_transition() {
        assert!(!should_emit_state(
            DownloadState::Progress,
            DownloadState::Progress
        ));
        assert!(should_emit_state(
            DownloadState::Progress,
            DownloadState::Completed
        ));
        assert!(should_emit_state(
            DownloadState::Progress,
            DownloadState::Failed
        ));
        assert!(should_emit_state(
            DownloadState::Progress,
            DownloadState::Canceled
        ));
        // 已结束不再重复广播(终态 sticky)
        assert!(!should_emit_state(
            DownloadState::Completed,
            DownloadState::Failed
        ));
        assert!(!should_emit_state(
            DownloadState::Canceled,
            DownloadState::Completed
        ));
    }

    /// finish():第一次跃迁返回 true,后续(含同类终态)返回 false
    #[test]
    fn finish_reports_transition_only_once() {
        let dir = temp_dir("emit-finish");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, _) = downloads.begin("http://example.com/a.bin", "a.bin");
        assert!(downloads.finish(&id, DownloadState::Completed, None));
        assert!(!downloads.finish(&id, DownloadState::Failed, Some("late".into())));
        assert!(!downloads.finish(&id, DownloadState::Completed, None));
        // 未知 id 不广播
        assert!(!downloads.finish("dl-unknown", DownloadState::Failed, None));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 取消只算一次跃迁:取消后采集层的终态回调不再触发广播
    #[test]
    fn cancel_is_the_only_transition_after_progress() {
        let dir = temp_dir("emit-cancel");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/big.bin", "big.bin");
        fs::write(&path, b"partial").unwrap();
        downloads.cancel(&id).unwrap();
        assert!(!downloads.finish(&id, DownloadState::Failed, None));
        assert!(!downloads.finish(&id, DownloadState::Completed, None));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 删除:移除记录并删除磁盘文件
    #[test]
    fn delete_removes_record_and_file() {
        let dir = temp_dir("delete-ok");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/a.bin", "a.bin");
        fs::write(&path, b"bytes").unwrap();
        downloads.finish(&id, DownloadState::Completed, None);

        let value = downloads.delete(&id).unwrap();
        assert_eq!(value["deleted"], json!(true));
        assert_eq!(value["filename"], json!("a.bin"));
        assert!(value.get("error").is_none(), "成功时不该带 error");
        assert!(!path.exists(), "磁盘文件应被删除");
        assert_eq!(record_count(&downloads), 0, "记录应被移除");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 删除:进行中拒绝,记录与文件都保留
    #[test]
    fn delete_rejects_in_progress_download() {
        let dir = temp_dir("delete-progress");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/big.bin", "big.bin");
        fs::write(&path, b"partial").unwrap();

        assert_eq!(downloads.delete(&id).unwrap_err(), DELETE_PROGRESS_ERROR);
        assert!(downloads.is_progress(&id), "进行中的记录必须保留");
        assert!(path.exists(), "进行中的文件不能被删");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 删除:文件已不存在仍移除记录,响应 `deleted:false` + 原因
    #[test]
    fn delete_missing_file_still_drops_record() {
        let dir = temp_dir("delete-missing");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, path) = downloads.begin("http://example.com/gone.bin", "gone.bin");
        downloads.finish(&id, DownloadState::Failed, Some("interrupted".into()));
        assert!(!path.exists(), "该测试要求文件本来就不存在");

        let value = downloads.delete(&id).unwrap();
        assert_eq!(value["deleted"], json!(false));
        assert_eq!(value["filename"], json!("gone.bin"));
        assert_eq!(value["error"], json!("file not found"));
        assert_eq!(record_count(&downloads), 0, "记录仍应被移除");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 删除:未知 id 报错且不动任何记录
    #[test]
    fn delete_unknown_id_errors() {
        let dir = temp_dir("delete-unknown");
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        let (id, _) = downloads.begin("http://example.com/keep.bin", "keep.bin");
        downloads.finish(&id, DownloadState::Completed, None);

        let err = downloads.delete("dl-404").unwrap_err();
        assert!(err.contains("dl-404"), "错误文案应带 id: {err}");
        let snapshot = downloads.snapshot(None);
        assert_eq!(snapshot["groups"][0]["items"].as_array().unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 完成事件 payload:带 filename 与 path(前端 toast「已下载」用)
    #[test]
    fn completed_event_payload_carries_filename_and_path() {
        let payload = download_completed_payload("dl-9", "pic.png", r"D:\dl\pic.png");
        assert_eq!(
            payload,
            json!({
                "id": "dl-9",
                "filename": "pic.png",
                "path": r"D:\dl\pic.png",
                "state": "completed",
            })
        );
    }

    /// 回填:已完成/已取消原样恢复,`progress` → `failed` + 重启文案,id 序号接着排
    #[test]
    fn history_load_backfills_and_marks_progress_failed() {
        let dir = temp_dir("history-load");
        let history = dir.join(HISTORY_FILE_NAME);
        fs::create_dir_all(history.parent().unwrap()).unwrap();
        let done = dir.join("done.bin").to_string_lossy().to_string();
        let running = dir.join("running.bin").to_string_lossy().to_string();
        let canceled = dir.join("canceled.bin").to_string_lossy().to_string();
        let entries = json!([
            {
                "id": "dl-0",
                "url": "http://example.com/done.bin",
                "filename": "done.bin",
                "path": done,
                "totalBytes": 10,
                "receivedBytes": 10,
                "state": "completed",
                "startedAt": 1000,
            },
            {
                "id": "dl-7",
                "url": "http://example.com/running.bin",
                "filename": "running.bin",
                "path": running,
                "totalBytes": 100,
                "receivedBytes": 42,
                "state": "progress",
                "kind": SAVE_AS_KIND,
                "startedAt": 2000,
            },
            {
                "id": "dl-3",
                "url": "http://example.com/canceled.bin",
                "filename": "canceled.bin",
                "path": canceled,
                "totalBytes": 0,
                "receivedBytes": 0,
                "state": "canceled",
                "startedAt": 1500,
            }
        ]);
        fs::write(&history, serde_json::to_vec(&entries).unwrap()).unwrap();

        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history.clone());
        downloads.restore();

        assert_eq!(record_count(&downloads), 3, "三条都应回填");
        let snapshot = downloads.snapshot(None);
        let items = snapshot["groups"][0]["items"].as_array().unwrap();
        let running = items.iter().find(|i| i["id"] == json!("dl-7")).unwrap();
        assert_eq!(running["state"], json!("failed"), "上次被杀 → failed");
        assert_eq!(running["error"], json!(INTERRUPTED_ERROR));
        assert_eq!(running["receivedBytes"], json!(42), "已收字节保留");
        assert_eq!(running["kind"], json!(SAVE_AS_KIND), "kind 保留");
        let done = items.iter().find(|i| i["id"] == json!("dl-0")).unwrap();
        assert_eq!(done["state"], json!("completed"), "已完成不该被改");
        assert!(done.get("error").is_none(), "无错误时不该出现 error 字段");
        let canceled = items.iter().find(|i| i["id"] == json!("dl-3")).unwrap();
        assert_eq!(canceled["state"], json!("canceled"));

        // 回填后新 id 不能与已有 id 撞车(dl-7 之后的序号是 8)
        let (new_id, _) = downloads.begin("http://example.com/new.bin", "new.bin");
        assert_eq!(new_id, "dl-8");

        // 归档形状与 /api/downloads 一致(camelCase 且带齐必要字段)
        for key in [
            "id",
            "url",
            "filename",
            "path",
            "totalBytes",
            "receivedBytes",
            "state",
            "startedAt",
        ] {
            assert!(done.get(key).is_some(), "回填 item 缺字段 {key}");
        }

        // 修正确实会落盘(重启后再读到 failed,而不是又变回 progress)
        downloads.flush_history();
        let text = fs::read_to_string(&history).unwrap();
        let saved: Value = serde_json::from_str(&text).unwrap();
        assert!(saved.as_array().unwrap().iter().any(|i| {
            i["id"] == json!("dl-7") && i["state"] == json!("failed")
        }));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 新路径读写:历史落在 `<sessionDataDir>/download-history.json`;
    /// `downloads/` 下不再有历史文件
    #[test]
    fn history_persists_at_session_root_not_download_dir() {
        let dir = temp_dir("history-new-path");
        let history = dir.join(HISTORY_FILE_NAME);
        let mut downloads = Downloads::new(dir.clone(), Some(dir.clone()), 64);
        downloads.set_history_path(history.clone());
        downloads.begin("http://example.com/a.bin", "a.bin");
        assert!(downloads.flush_history());
        assert!(history.exists(), "历史应写在会话数据目录根下");
        let saved: Value = serde_json::from_slice(&fs::read(&history).unwrap()).unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 1);
        assert!(
            !dir.join(LEGACY_HISTORY_SUBDIR)
                .join(LEGACY_HISTORY_FILE_NAME)
                .exists(),
            "下载目录下不应有历史文件"
        );

        // 重启按新路径回填
        let mut restarted = Downloads::new(dir.clone(), Some(dir.clone()), 64);
        restarted.set_history_path(history);
        restarted.restore();
        assert_eq!(record_count(&restarted), 1, "新路径应能正常回填");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 旧路径 → 新路径一次性迁移:内容搬移且旧文件被删除
    #[test]
    fn legacy_history_migrates_to_new_path_and_removes_old() {
        let dir = temp_dir("history-migrate");
        let legacy = dir
            .join(LEGACY_HISTORY_SUBDIR)
            .join(LEGACY_HISTORY_FILE_NAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        let entries = json!([{
            "id": "dl-0",
            "url": "http://example.com/a.bin",
            "filename": "a.bin",
            "path": dir.join("a.bin").to_string_lossy(),
            "totalBytes": 3,
            "receivedBytes": 3,
            "state": "completed",
            "startedAt": 10,
        }]);
        fs::write(&legacy, serde_json::to_vec(&entries).unwrap()).unwrap();

        let new_path = dir.join(HISTORY_FILE_NAME);
        assert!(migrate_legacy_history(&new_path, &legacy));
        assert!(new_path.exists(), "新文件应被创建");
        assert!(!legacy.exists(), "迁移后旧文件应被删除");
        let saved: Value = serde_json::from_slice(&fs::read(&new_path).unwrap()).unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 1, "内容应原样搬移");

        // 迁移后的内容能被正常回填
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(new_path);
        downloads.restore();
        assert_eq!(record_count(&downloads), 1);

        // 再跑一次(新文件已在)→ 空操作
        assert!(!migrate_legacy_history(
            &dir.join(HISTORY_FILE_NAME),
            &legacy
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 新文件已存在 → 不覆盖,旧文件保持原样
    #[test]
    fn legacy_migration_keeps_existing_new_file() {
        let dir = temp_dir("history-migrate-keep");
        let legacy = dir
            .join(LEGACY_HISTORY_SUBDIR)
            .join(LEGACY_HISTORY_FILE_NAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, br#"[{"id":"dl-old","path":"C:\\old.bin"}]"#).unwrap();
        let new_path = dir.join(HISTORY_FILE_NAME);
        fs::write(&new_path, br#"[{"id":"dl-new","path":"C:\\new.bin"}]"#).unwrap();

        assert!(!migrate_legacy_history(&new_path, &legacy));
        assert_eq!(
            fs::read_to_string(&new_path).unwrap(),
            r#"[{"id":"dl-new","path":"C:\\new.bin"}]"#
        );
        assert!(legacy.exists(), "未迁移时旧文件应保持原样");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 迁移失败容错:读/写失败都只记日志,保留旧文件,不 panic
    #[test]
    fn legacy_migration_tolerates_failures() {
        let dir = temp_dir("history-migrate-fail");
        let legacy = dir
            .join(LEGACY_HISTORY_SUBDIR)
            .join(LEGACY_HISTORY_FILE_NAME);
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, br#"[{"id":"dl-0","path":"C:\\a.bin"}]"#).unwrap();

        // 写新文件失败:父路径是个普通文件 → create_dir_all 报错
        let blocker = dir.join("blocked");
        fs::write(&blocker, b"file").unwrap();
        let blocked_new = blocker.join(HISTORY_FILE_NAME);
        assert!(!migrate_legacy_history(&blocked_new, &legacy));
        assert!(legacy.exists(), "写失败时旧文件必须保留");
        assert!(!blocked_new.exists());

        // 读旧文件失败:旧路径是个目录
        let legacy_dir = dir.join("legacy-dir");
        fs::create_dir_all(&legacy_dir).unwrap();
        assert!(!migrate_legacy_history(
            &dir.join(HISTORY_FILE_NAME),
            &legacy_dir
        ));
        assert!(!dir.join(HISTORY_FILE_NAME).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// 坏 JSON / 非数组 / 非法条目:按空列表开跑,不 panic
    #[test]
    fn history_bad_json_falls_back_to_empty_list() {
        let dir = temp_dir("history-bad");
        let history = dir.join(HISTORY_FILE_NAME);
        fs::create_dir_all(history.parent().unwrap()).unwrap();

        fs::write(&history, b"{ not json").unwrap();
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history.clone());
        downloads.restore();
        assert_eq!(record_count(&downloads), 0, "坏 JSON → 空列表");

        fs::write(&history, b"{\"items\":[]}").unwrap();
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history.clone());
        downloads.restore();
        assert_eq!(record_count(&downloads), 0, "非数组 → 空列表");

        // 缺 id / path 的条目跳过,合法的保留
        let entries = json!([
            { "url": "http://example.com/x", "state": "completed" },
            { "id": "dl-1", "path": dir.join("ok.bin").to_string_lossy(), "state": "completed" }
        ]);
        fs::write(&history, serde_json::to_vec(&entries).unwrap()).unwrap();
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history);
        downloads.restore();
        assert_eq!(record_count(&downloads), 1, "只保留合法条目");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 防抖 + 原子写:窗口内不写、到点写一次、临时文件不残留
    #[test]
    fn history_debounce_and_atomic_write() {
        let dir = temp_dir("history-debounce");
        let history = dir.join(HISTORY_FILE_NAME);
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history.clone());

        let changed_at = now_ms();
        downloads.begin("http://example.com/a.bin", "a.bin");

        // 防抖窗口内:不写盘(即使有变更)
        assert!(downloads.history_pending(), "应有待写变更");
        assert!(
            !downloads.flush_if_due(changed_at + HISTORY_DEBOUNCE_MS - 1),
            "未到防抖窗口不该写"
        );
        assert!(!history.exists(), "不应该提前产生 history.json");

        // 到点:写一次,内容可解析
        assert!(downloads.flush_if_due(now_ms() + HISTORY_DEBOUNCE_MS + 1));
        let saved: Value = serde_json::from_slice(&fs::read(&history).unwrap()).unwrap();
        assert_eq!(saved.as_array().unwrap().len(), 1);
        assert_eq!(saved[0]["filename"], json!("a.bin"));
        assert!(!downloads.history_pending(), "写完后不应还有待写变更");
        assert!(
            !downloads.flush_if_due(now_ms() + 10 * HISTORY_DEBOUNCE_MS),
            "无变更时不该重复写"
        );

        // 原子写:临时文件不在(rename 已消费),目录里只有 history.json
        let names: Vec<String> = fs::read_dir(history.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec![HISTORY_FILE_NAME.to_string()], "got {names:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 防抖合并:窗口内多次变更只落盘一次,且内容是最新状态
    #[test]
    fn history_debounce_merges_changes_into_one_write() {
        let dir = temp_dir("history-merge");
        let history = dir.join(HISTORY_FILE_NAME);
        let mut downloads = Downloads::new(dir.clone(), None, 64);
        downloads.set_history_path(history.clone());

        let ids: Vec<String> = (0..3)
            .map(|i| {
                downloads
                    .begin("http://example.com/x.bin", &format!("x{i}.bin"))
                    .0
            })
            .collect();
        downloads.finish(&ids[0], DownloadState::Completed, None);
        downloads.finish(&ids[1], DownloadState::Failed, Some("boom".into()));
        assert!(downloads.flush_if_due(now_ms() + HISTORY_DEBOUNCE_MS + 1));

        let saved: Value = serde_json::from_slice(&fs::read(&history).unwrap()).unwrap();
        let items = saved.as_array().unwrap();
        assert_eq!(items.len(), 3, "三次 begin 合并成一次写");
        let failed = items.iter().find(|i| i["id"] == json!(&ids[1])).unwrap();
        assert_eq!(failed["state"], json!("failed"));
        assert_eq!(failed["error"], json!("boom"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// 落盘时机:静默满防抖窗口就写;持续变更(下载进度)时按上限兜底写
    #[test]
    fn history_write_due_covers_debounce_and_max_wait() {
            assert!(!history_write_due(1_000, 900, 0), "刚变更过不该写");
            assert!(history_write_due(1_500, 1_000, 0), "静默满 500ms 该写");
        // 进度每 100ms 刷新一次:静默条件永不满足,靠待写上限兜底
        assert!(!history_write_due(2_900, 2_800, 1_000));
        assert!(history_write_due(3_000, 2_900, 1_000));
    }

    /// 另存为来源判定:HTML → save-page;图片/无信息 → save-as
    #[test]
    fn save_kind_splits_page_saves_from_other_saves() {
        assert_eq!(classify_save_kind("text/html", ""), SAVE_PAGE_KIND);
        assert_eq!(
            classify_save_kind("application/xhtml+xml; charset=utf-8", "page.html"),
            SAVE_PAGE_KIND
        );
        assert_eq!(classify_save_kind("", "index.html"), SAVE_PAGE_KIND);
        assert_eq!(classify_save_kind("", "page.HTM"), SAVE_PAGE_KIND);
        assert_eq!(classify_save_kind("image/png", "pic.png"), SAVE_AS_KIND);
        // WebView2 对图片另存也回报 text/html:扩展名优先,不能被误判成整页另存
        assert_eq!(
            classify_save_kind("text/html", "3.png"),
            SAVE_AS_KIND,
            "mime 不可靠时以扩展名为准"
        );
        assert_eq!(classify_save_kind("application/pdf", ""), SAVE_AS_KIND);
        assert_eq!(classify_save_kind("", ""), SAVE_AS_KIND, "判不出来按 save-as");
    }
}
