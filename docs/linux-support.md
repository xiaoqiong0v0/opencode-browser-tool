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
| 网络响应捕获 / 可访问性树 / 新窗口拦截 | ❌ | 依赖 WebView2 原生事件，Linux 未实现（对应模块整体 `#[cfg(windows)]`） |
| 运行时改 User-Agent | ❌ | 同上（依赖 `ICoreWebView2Settings2`） |
| 媒体权限放行 | ❌ | 依赖 WebView2 `PermissionRequested` 事件 |
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
