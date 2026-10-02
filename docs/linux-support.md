# Linux 支持说明（独立变更文档）

> 本文档记录 V5 架构在 **Linux** 上的适配结论与约定。原始架构文档 `v5-tauri-architecture.md` 不改，变更在此独立说明。
> 状态：Linux 主路径已在 **WSLg（Kali）** 上实测通过；真实 Linux 桌面（GNOME/KDE）未实测；macOS 未实现。

## 1. 现状总览

| 能力 | Linux | 说明 |
|---|---|---|
| 编译 | ✅ | 平台专属模块用 `#[cfg(windows)]` 隔离（关键点：`control/responses.rs` 曾顶层 `use webview2_com`，是唯一编译阻塞点） |
| 多 Webview 布局 | ✅ | 见 §2 垫片 |
| 窗口缩放（四边/四角） | ✅ | 自绘热区 + `begin_resize_drag`（Wayland→`xdg_toplevel.resize`，X11→WM） |
| 覆盖层透明 / 面板遮罩 / 批注 | ✅ | 依赖窗口 `transparent`，见 §3.1 |
| 页面截图 | ✅ | WebKitGTK `WebView::snapshot`（见 §4） |
| 开发者工具 | ✅ | 开关状态用 wry 的真实 `is_devtools_open()`（Windows 上 wry 该值为恒 false，需另用本地标记） |
| 网络响应捕获 / 新窗口拦截 | ✅ | 见 §6 |
| 可访问性树 | ⚠️ 近似 | 见 §6（ATK 需 a11y bus，精简环境不可用） |
| 运行时改 User-Agent | ✅ | `WebKitSettings::set_user_agent`（仅当前页面 Webview） |
| 媒体权限放行 | ✅ | `permission-request` 信号 → `UserMediaPermissionRequest.allow()` |
| macOS | ❌ | 未实现（截图等仍为 stub） |

## 2. 布局垫片 `ui/linux_layout.rs`（核心）

**为什么需要**：`tauri-runtime-wry` 在 Linux 把子 Webview `pack_start` 进窗口的 `gtk::Box`（Vertical, expand），而 wry 的 `set_bounds` 只在容器为 `gtk::Fixed` 时才 `size_allocate` —— 因此 `Webview::set_position/set_size` 在 Linux **静默失效**，多个子 Webview 会均分窗口高度（上游 issue `tauri#10420` 至今未修，修复 PR `#15704/#15463` 未合并）。

**做法**：用 `with_webview(|pw| pw.inner())` 拿到 `webkit2gtk::WebView`，自建 `gtk::Fixed` 挂到窗口的 GtkBox，把全部子 Webview `reparent` 进去（`Fixed::move_` + `Widget::set_size_request` 绝对定位）。

模块提供的公共入口：

| 函数 | 作用 |
|---|---|
| `reparent(app, labels)` | 把子 Webview 从 GtkBox 迁入自建 `Fixed`（幂等） |
| `place(app, label, x, y, w, h)` | 绝对定位（`move_` + `set_size_request`） |
| `raise(app, label)` | remove+put 置顶（**只在"显示浮层"时调用一次**，勿每帧调用） |
| `poke(app, label)` | GTK 侧主动重排+重绘（`queue_resize`/`queue_draw`，含顶层） |
| `BtFixed` | `gtk::Fixed` 的 Rust 子类，**只把自身 preferred size 谎报为 (1,1)** —— 否则子控件的 `size_request` 会把容器 minimum 顶成"当前窗口尺寸"，导致窗口**只能放大不能缩小** |

**配套约定（`ui/mod.rs`）**：

- 所有"显示子 Webview"的地方统一走 `show_webview(app, label)`：`show()` 后 Linux 额外 `poke` + `kick_render`（WebKit 渲染进程只在 DOM 变化/输入/尺寸变化时出帧，否则新显示的 Webview 白屏、要等鼠标移入才绘制；并由 `kick_render` 在 250/800/1600ms 延迟补驱动，覆盖冷启动页面尚未加载完的情况）。
- 覆盖层/面板**用显式 `show()`/`hide()` 管理可见性**，不要靠"宽度归零"隐藏：Wayland 下宽度归零后输入区域不会随 resize 恢复，表现为"可见但点不动、事件穿透到下层页面"。
- `GtkFixed` 中子控件顺序即 z 序；动态新建标签的页面 Webview 会盖住先创建的浮层，故显示浮层时 `raise` 一次即可（切换/新建标签都会先关闭面板与模式）。

## 3. 硬性约定与禁忌

1. **不要设置** `WEBKIT_DISABLE_DMABUF_RENDERER` / `WEBKIT_DISABLE_COMPOSITING_MODE`：实测它们会破坏透明合成，导致覆盖层渲染成不透明黑块或整片透出桌面（曾误判为"WSLg 不支持透明"，真因就是这两个变量）。
2. **不要开启 CSD 装饰**（`decorations(true)`）来换取缩放：内容 preferred 尺寸随窗口增长会形成反馈循环，窗口会失控增大（实测涨到 3908×5999）并触发 `Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display` 崩溃；且与工具栏自绘的窗口按钮重复。Linux 缩放一律用 `linux_layout` 的热区。
3. **图标一律用 lucide 内联 SVG**，禁止用 emoji / 文本符号当图标（依赖 emoji/符号字体，精简发行版上必然显示成豆腐块；本项目早期用过 🤖🛠🌐 与 ─□✕ 等）。
4. **favicon 从页面自身取**（`link[rel~=icon]` → 回退 `origin + /favicon.ico`，随标签状态 payload 的 `icon` 字段下发），不要依赖第三方 favicon 服务（国内/无代理环境取不到）。
5. Linux 下窗口需 `transparent(true)`（RGBA visual + app_paintable）覆盖层透明才成立；`decorations(false)` 保持不变。

## 4. 截图实现（Linux）

WebKitGTK：`WebViewExt::snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE, Cancellable::NONE, cb)`。
注意：

- `SnapshotRegion` **只有 `Visible` / `FullDocument`，没有矩形参数** → 区域截图要"先截 Visible，再用 cairo 偏移 `set_source_surface` + `paint` 裁剪"。
- PNG 编码需 `cairo-rs` 且**必须显式开 `features = ["png"]`**（gtk 0.18 未启用）；base64 用 `base64` crate。
- 调用方都在后台线程，`with_webview` 在非主线程只投递消息到 GTK 主线程，因此后台线程阻塞 `recv_timeout` 不会死锁。

## 5. 构建与环境

- `rust/Cargo.toml` 增加 Linux target 依赖：`gtk = "0.18"`、`webkit2gtk = "2.0"`、`cairo-rs = { version = "0.18", features = ["png"] }`、`base64 = "0.22"`（版本需与 Cargo.lock 对齐）。
- 系统依赖（Debian 系）：`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev build-essential`，另需 `cmake clang libclang-dev`（aws-lc-sys 等绑定生成）。
- 字体：精简发行版（Kali）默认无 emoji/CJK 字体；即便按 §3.3 改用 SVG 图标，**中文文本仍依赖系统 CJK 字体**（桌面发行版通常自带）。
- WSLg 注意事项：应用走 Wayland 原生，**X11 抓屏工具（scrot/import/xwd）抓不到内容**（全黑），`xwininfo` 也看不到窗口；`xdotool` 无法验证 WM 动作（合成事件走 XWayland 内部，WM 动作在 Wayland 侧）；反复创建/销毁窗口后 WSLg/Weston 状态可能变脏（窗口已注册但 Windows 侧不显示）→ `wsl --shutdown` 复位。
- 调试建议：任意可编程验证尽量走本项目自身的 HTTP API（`/api/status`、`/api/evaluate`、`/api/screenshot`），比截图/看界面可靠。

## 6. 平台能力的 Linux 实现（替代 WebView2 原生事件）

| 能力 | Windows（原实现） | Linux（WebKitGTK） |
|---|---|---|
| 新窗口拦截 | `NewWindowRequested` 事件 | `WebView::connect_create` → 恒返回 `None` 拒绝原生开窗，改调本项目 `open_new_tab`（复用 Windows 同一入口与 `should_skip()` 过滤：about:blank + 800ms 去重） |
| 媒体权限放行 | `PermissionRequested`（仅 CAMERA/MICROPHONE） | `connect_permission_request` → `dynamic_cast_ref::<UserMediaPermissionRequest>()` 成功后 `allow()` 并返回已处理；其余保持默认 |
| 运行时改 UA | `ICoreWebView2Settings2.SetUserAgent` | `widget.settings()` → `SettingsExt::set_user_agent(Some(ua))`（`settings()` 返回 `Option<Settings>`，None 视为失败；沿用 mpsc + 5s 超时回传结构） |
| 响应捕获 | `WebResourceResponseReceived` 事件 | `connect_resource_load_started` → 对 `WebKitWebResource` 监听 `notify::response` → `URIResponseExt::uri()/status_code()` 写入同一 `RESPONSE_LOG`（上限 500、最新在末尾；`load_started` 时响应通常未就绪，故必须先监听 notify） |
| 可访问性树 | CDP `Accessibility.getFullAXTree` | **近似实现**：`ui::eval_page` 执行内嵌 JS（`DOM_AX_JS`）从 DOM 重建同结构树（显式/隐式 role、accessible name 优先级、表单 value、跳过不可见与非内容元素、沿用 Windows 侧"generic 且无 name 则提升子树"的过滤） |

**为什么可访问性树用近似而非 ATK**：`webkit_web_view_get_accessible()` 依赖 ATK + a11y bus（`at-spi2`），在 WSLg/精简发行版上通常不可用，无法验证与运行；DOM 近似在任何环境都可用，且对"元素定位/标注"这类用途足够。

**已知限制**：近似树不是浏览器真实 AX 树（不含 layout/table 语义细节，name 计算也与浏览器实现有差异）；`data:`/`blob:` 等非 HTTP 资源的响应 status 可能记为 0。

## 7. 平台能力与降级策略（交互命令）

### 7.1 统一规则

1. **有可信通道就用可信通道**：Windows/WebView2 → CDP（`Input.*` / `DOM.setFileInputFiles`）；Linux/WebKitGTK → GDK `gdk_event_put` 注入原生键盘/指针事件。两者都产生 `isTrusted=true` 事件、真实命中测试与默认行为。
2. **没有可信通道（macOS）时，退回原本"确实可用"的 DOM/JS 实现，但必须标注降级**：service 返回 `{degraded:true, degradedReason:"untrusted JS fallback on <os>"}`，插件 `src/index.ts` 统一格式化为 ` (degraded: ...)`；**绝不静默假成功**。
3. **原本就不可用/假成功的实现不做兜底**：保持明确报错（宁可报错也不假成功）。
4. **"原本是否确实可用"以旧实现行为与实测为准**；不确定处标注存疑。

调试开关：环境变量 **`BT_FORCE_DEGRADED=1`** 会在 Windows/Linux 上强制走降级分支，用于实测降级路径与标注文本。判定入口在 `rust/src/control/mod.rs`（`trusted_available()` / `Outcome`）。

### 7.2 Linux 可信输入实现（GDK）

`rust/src/control/gdk.rs`：在 GTK 主线程（`webview.with_webview`）用 `gdk_event_put` 注入合成 `GdkEvent`（键盘 `GdkEventKey`、指针 `GdkEventMotion`/`GdkEventButton`），由 GTK 分发给 `WebKitWebView`（等于原生事件）。事件的 `window` 指向 webview 的 GdkWindow；若 webview 没有独立 GdkWindow（事件窗口是 toplevel），坐标会自动加上 webview 原点偏移。取证过程与实测输出见 `.tmp/scratch/linux-gdk-evidence.md`。

关键实测结论（WSLg，页面侧读取）：

- 键盘：单字符输入、Enter 提交、方向键、Tab 全部 `isTrusted=true` 且默认行为生效；
- 指针：点击命中/坐标、CSS `:hover` 均生效（验证时目标须先在视口内，越界坐标会被丢弃）；
- 文本：`fill` 用 GDK 逐字符注入，`gdk_unicode_to_keyval` 支持大小写/数字/符号。

**异步读回**：WebKitGTK 的输入事件经 GTK → WebKit UI 进程 → Web 进程异步处理，`gdk_event_put` 后立即 `eval` 读回会读到旧值，故 `input.rs` 的 `fill`/`clear`/`select`/`iframe_fill` 读回改为**轮询等待**（最长 2.5s）；Windows 同步路径首次读取即命中，不受影响。

### 7.3 命令级现状

| 命令 | Windows（可信，CDP） | Linux（WebKitGTK） | 现状与限制 |
|---|---|---|---|
| `click` | `Input.dispatchMouseEvent`（moved→pressed→released） | **可信**：GDK motion+press+release | 两平台派发前均做命中测试/视口/disabled 校验；被遮挡/出视口/disabled → 明确报错 |
| `hover` | `mouseMoved` | **可信**：GDK motion | 实测触发 CSS `:hover`；强制降级分支不触发 |
| `click_and_switch_tab` | 同 `click` | **可信**：同 `click` | Rust 侧只负责点击 + 200ms 后读 URL；"切标签"逻辑不在 Rust 侧 |
| `iframe_click` | 内容坐标换算到顶层视口后 CDP 点击 | **可信**：同 Windows（GDK） | 仅支持顶层选择器定位的**单层 iframe**；跨域 iframe 两平台都明确报错 |
| `fill` | focus → 全选 → `Input.insertText` → 轮询读回 | **可信**：focus → 全选 → GDK 逐字符按键 → 轮询读回 | 产生可信 `beforeinput`/`input`（受控组件可用）；读回超时不一致即报错 |
| `clear` | focus → 全选 → `Delete` → 读回 | **可信**：focus → 全选 → GDK `Delete` → 读回 | |
| `select` | focus → 方向键移动 → 读回 | **可信**：同 Windows（GDK 方向键） | `multiple`/`size>1` 无可信键盘路径 → **两平台降级并标注**；目标 value 不存在 → 报错 |
| `iframe_fill` | iframe 内 focus → `insertText` → 读回 | **可信**：同 Windows（GDK 逐字符） | |
| `press_key` | `Input.dispatchKeyEvent` | **可信**：GDK `GdkEventKey` | macOS 无可信通道 → 明确报错 |
| `upload_file` | `DOM.setFileInputFiles` + 读回 `el.files` | **明确报错** | Linux 无 CDP；候选（WebDriver / JS File 注入）未确证；旧 `fetch("file:///…")` 是确定性假成功 |
| `drag` | `Input.dispatchMouseEvent` 鼠标序列（原生 HTML5 DnD） | **明确报错** | Linux 上 GDK 鼠标序列能否触发原生 DnD **未验证**，不做未确证实现；旧合成 `DragEvent` 基本不可用 |

> - 未列出的交互/只读命令（`scroll`、`scroll_to_element`、`get_*`、`evaluate` 等）在各平台均为可用实现，不涉及降级/报错。
> - `drag`（Windows）未采用 `Input.dispatchDragEvent` + `setInterceptDrags`：实测普通鼠标序列已能触发可信原生 DnD，且本项目 CDP 通道是请求/响应式、收不到 `Input.dragIntercepted` 事件。
> - macOS：无 CDP / GDK 通道 → 鼠标/文本类命令走"降级 + 标注"，键盘/上传/拖拽明确报错（由 `trusted_available()` 与各命令平台分支决定）。

### 7.4 时序/就绪（平台无关）

`navigate` / `go_back` / `go_forward` 不再固定 sleep，而是等待就绪信号：`document.readyState` 达到
`interactive`/`complete`，或观察到导航进展（URL 变化 / 曾进入 `loading`）；`scroll` 等待窗口滚动位置连续多次
采样不变，`scroll_to_element` 额外校验元素真的进入视口（主路径用**立即** `scrollIntoView`，避免平滑滚动
在嵌套滚动容器上停在中途；必要时兜底逐级滚动可滚动祖先 + 窗口）。均有**有界超时**（默认 navigate 15s、history 5s、
scroll 3s），超时**如实回传** `ready:false` / `stable:false` / `reached:false` + `timedOut:true` + `waitedMs`，
`scroll_to_element` 不可达时另带 `reason`，插件在结果文本后追加说明，不假装成功。

调试覆盖（起实例时设置）：`BT_NAV_TIMEOUT_MS`、`BT_HISTORY_TIMEOUT_MS`、`BT_SCROLL_TIMEOUT_MS`。
Kali 复验步骤见 `linux-verification.md`。
