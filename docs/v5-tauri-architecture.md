# V5 架构：Tauri 2 跨平台多 Webview（方案变更说明）

> **⚠️ 历史设计文档（v5 立项时）**：本文保留当时的方案与里程碑，**现状以 `README.md` / `docs/implementation.md` / `docs/linux-support.md` / `docs/ASYNC.md` 为准**。已知与现状不一致处（正文相应位置已就地更正，此处汇总）：
> - **截图**：macOS 仍是 stub（`rust/src/control/screenshot.rs` 非 Windows/Linux 分支直接返回 `Err`），**不存在**「WKWebView snapshot」能力；Linux 用 WebKitGTK `WebView::snapshot`。
> - **发送链路**：不是「插件每 2s 轮询」，改为 **shell 主动 POST `/notify/<sid>` + 服务就绪回调触发事件驱动 drain**（`src/index.ts:setupAnnotateDelivery` / `drainAnnotate`，投递 worker 见 `rust/src/ui/mod.rs:notify_plugin`）。
> - **截图不进对话 part**：服务端对 `image/*` part 返回 400，改为**落盘 + 文本路径**，图片经工具结果附件（`screenshot` / `read_record_content`）给模型。
> - **发送后不清空记录**：只置 `sent=true` 保留（`rust/src/ui/mod.rs:send_all_records`，上限 `MAX_RECORDS=200`）。
> - **工具数**：插件只注册 **2 个工具**（`bt_cli` CLI 分发器 + `bt_set_media_mode`）；`COMMANDS` 表是 `bt_cli` 的**子命令**（当前 47 条），不是 42/47 个工具。
> - **设备预设**：14 个（不含面板首项「默认」`__default__`）。
> - **鉴权**：所有 `/api/*` 需 `x-bt-token`（唯一精确路径豁免 `/api/page-event`），本文成文时还没有这套令牌。

> 变更原因：V4 方案（WebView2 + DirectComposition + Win32）完全绑定 Windows。
> 用户决策：直接换跨平台框架，确认走 **Tauri 2 多 Webview** 路线。
> 覆盖层方案：透明 Webview 叠加（非平台原生合成）。

## 目标形态（单窗口 + 多 Webview）

```
┌── Tauri 窗口(单窗口,无标题栏) ────────────────────┐
│  ├─ toolbar Webview (顶部: 标签行 + 导航行)        │
│  │    └ 标签多→ ◀▶滚动 / + 新建 / 窗口按钮          │
│  │    └ 导航行: ←→⟳ 地址栏 [✏批注][📷截图][⚙面板] │
│  ├─ page-1..N Webview(真多标签,渲染目标网页)       │
│  ├─ overlay Webview(透明叠加,批注/截图选区交互)     │
│  ├─ panel Webview(右侧覆盖式浮层,AI+配置)          │
│  └─ newtab(内部页,新标签页,跟随主题)                │
└─────────────────────────────────────────────────────┘
        ↑ Rust 后端: 状态机 + eval 协议 + WebView2 原生事件
opencode 插件(client.ts) ← HTTP → Rust 服务(axum/axum)
```

- **单窗口多 Webview**：Tauri 2 `WebviewBuilder` + `window.add_child()`
- **真多标签**：每标签一个独立 page Webview（预创建 3 个 + 动态创建），切换 hide/show 不重载；动态创建的 webview 会盖在 overlay/panel 之上，Windows 用 `SetWindowPos(HWND_TOP)` 把 overlay/panel 置顶
- **页面零注入**：页面 Webview 加载目标网页，控制通过 `eval_with_callback`（执行一次性 JS 查询/操作，不留驻脚本）
- **覆盖层**：透明 Webview（`transparent(true)`），Canvas 自绘高亮框/批注标记/截图选区，叠加在页面 Webview 上
- **面板**：普通 Webview，HTML/CSS/JS 实现（双 tab：AI 功能区 / 配置区）
- **新窗口拦截**（Windows）：WebView2 `NewWindowRequested` 原生事件 → 打开为新标签（过滤约800ms去重，target=_blank/window.open 均拦截）

## 平台能力对照

| 能力 | Windows | Linux | macOS |
|------|---------|-------|-------|
| Webview 引擎 | WebView2 | WebKitGTK | WKWebView |
| 多 Webview | ✓ add_child | ✓ | ✓ |
| 透明 Webview | ✓ | ✓ | ✓(私有 API) |
| eval 控制 | ✓ | ✓ | ✓ |
| 底层访问 | `with_webview` → ICoreWebView2Controller | → WebKitWebView | → WKWebView |
| 截图 | CDP Page.captureScreenshot（支持 clip 区域） | WebKitGTK `WebView::snapshot`（仅 Visible/FullDocument 区域，矩形靠 cairo 裁） | ✗ **未实现**（stub：非 Win/Linux 分支返回 `Err`） |

> **macOS 列为立项时的预期，从未实现**：多 Webview / 透明 / eval 三行的 ✓ 均无代码依据，现状是「macOS 未实现（截图等仍为 stub）」，见 `linux-support.md` §1；可信输入降级与明确报错的口径见 `linux-support.md` §7。本表除截图行外不要按 macOS 已可用理解。

## 控制协议（统一 eval 层）

- `eval(js) -> Result<String>`：执行 JS，返回 JSON 序列化结果（跨平台一致）
- 页面操作全部通过 eval 完成：click/fill/select/hover/scroll/elementFromPoint/selector 生成
- 截图：平台分支（Windows CDP Page.captureScreenshot，支持 clip 参数截区域 / Linux WebKitGTK snapshot 区域裁剪）；**macOS 未实现**
- 导航事件：`on_navigation` / `on_page_load` / `on_new_window` 回调
- 网络响应捕获（Windows）：`add_WebResourceResponseReceived` 收集 URL/status 响应日志（expect/assert 工具读取）
- 可访问性树（Windows）：CDP `Accessibility.getFullAXTree` → `{role,name,value,children}` 嵌套结构

## 与 V4 差异

| 项 | V4（弃用） | V5（新） |
|----|-----------|---------|
| 渲染 | WebView2 + DirectComposition | Tauri 系统 WebView |
| 覆盖层 | D2D 合成 visual | 透明 Webview + Canvas |
| 面板 | Win32 控件 | HTML Webview（V3 迁移） |
| 窗口管理 | Win32 消息循环 | Tauri（tao）事件循环 |
| 平台 | 仅 Windows | Win/Linux/macOS 一套代码 |

## 里程碑

| 阶段 | 内容 | 状态 |
|------|------|------|
| V5.1 | Tauri 骨架：窗口 + 3 Webview 布局（页面/覆盖层/面板） | ✅ 三 Webview 布局渲染，页面桥注入生效 |
| V5.2 | 控制协议：eval 封装 + 页面操作（导航/点击/填写/文本/元素状态/查询） | ✅ 端到端验证通过 |
| V5.3 | 覆盖层：透明 Webview 批注交互（悬停高亮/点击标记/右键退出） | ✅ 批注链路验证通过（selector+rect 正确） |
| V5.4 | 面板：TS 前端（列表/发送/i18n 基础） | ✅ 面板 TS 模块 + Rust 命令接通，发送队列+插件端轮询推送 |
| V5.5 | HTTP 服务对接 + 插件集成 | ✅ 全链路 HTTP 端点回归通过 |
| V5.6 | 截图 Windows CDP 实现 | ✅ PNG base64 验证通过(30007 字节) |
| V5.7 | 窗口 UI：无标题栏 + 自定义窗口按钮、主题系统（auto/light/dark）、真多标签（每标签独立 webview）、newtab 新标签页、事件驱动状态同步 | ✅ |
| V5.8 | 批注/截图双模式：遮罩式选区裁剪（默认整图/框选调整/双击还原）、保存需说明、发送后清空记录、记录卡片化 + 截图缩略图、面板 toast 通知 | ✅ |
| V5.9 | 真多标签接 HTTP（tabs 真实列表/new/switch/close）、新窗口拦截（NewWindowRequested → 新标签）、z-order 置顶（SetWindowPos） | ✅ |
| V5.10 | 工具补齐：网络响应捕获（expect/assert，WebResourceResponseReceived）、可访问性树（Accessibility.getFullAXTree）、插件取字段修复 | ✅ |

## 实施要点

- 前端 TS 工程化：`frontend/src/*.ts`（index 面板 / overlay 覆盖层 / page-bridge 页面桥 / toolbar 工具栏 / newtab）+ esbuild 打包到 `frontend/dist`，HTML 模板与 TS 分离
- 控制协议关键点：`eval_with_callback` 会把 JS 返回值 JSON 序列化，JS 表达式直接返回对象，不要再包 JSON.stringify（否则双重编码）
- 覆盖层：透明 Webview（`transparent(true)`），批注模式 show（拦截鼠标）+ 普通模式 hide（页面交互），鼠标事件 emit `overlay-input` → Rust 状态机
- 页面桥：`initialization_script` 注入 `window.__btPage`（query/state），导航后仍生效
- 批注/截图记录：Rust `UiState.records` 持有（type/url/selector/rect/note/image），`records-changed` 事件推送面板
- HTTP 端口：client.ts 显式传 `--port`（release 模式 GUI 无控制台，不能解析 stdout），轮询就绪
- 截图（Windows）：`with_webview` 拿 ICoreWebView2Controller → CoreWebView2() → CDP Page.captureScreenshot（带 clip 截区域）；必须与 tauri 使用相同 webview2-com 版本（0.38.2 / windows 0.61.3），否则类型不兼容；completed 回调签名 `(windows::core::Result<()>, String)` → `windows::core::Result<()>`
- 发送（**现状实现，非本文当年的轮询方案**）：面板按钮 → invoke `panel_cmd(send-all)` / HTTP `/api/annotate/send` → `ui::send_all_records` 把未发送记录快照**追加**进 `sent_records` 队列（按记录 `index` 去重、上限 `SENT_QUEUE_MAX=500` 丢最旧）、只置 `sent=true` **不清空 records** → shell 侧 POST 插件的 `/notify/<sid>`（单线程 FIFO worker，首发 + 300/600/1200ms 退避重试、单次超时 2s）触发插件 `drainAnnotate(sid)`（另有服务就绪回调兜底 drain，**全仓库已无 `startAnnotatePoller`/定时轮询**）→ `session.prompt` 只发 **text part**：截图落盘后在文本里写图片路径，图片本身**不经 `session.prompt`**（服务端对 `image/*` part 返回 400），需要看图走工具结果附件（`screenshot`）或 `read_record_content` 返回缓存路径。并发契约见 `docs/ASYNC.md`
- 新窗口拦截（Windows）：`ICoreWebView2::add_NewWindowRequested`（宏生成事件处理器），过滤空/about:blank + 同一 URL 800ms 去重（一次点击可能触发多次），token 存 static 防 GC
- 响应捕获（Windows）：`ICoreWebView2_2::add_WebResourceResponseReceived`（需 cast），回调收集 URL/status 入静态日志（上限 500 条 FIFO）
- 可访问性树（Windows）：复用截图 CDP 通道调 `Accessibility.getFullAXTree`，解析时过滤 generic 容器节点自身但**透传其子树**（否则只剩根节点）
- 状态广播：`emit_tabs_changed` / `emit_mode_state`（app.emit 全局广播，工具栏/面板都收）；Mutex 加锁注意提前 drop 避免重入死锁
- 清理：删除 Node 过渡代码（src/browsers、src/cdp、src/service、src/panel、src/bridge）、Rust 旧模块（cdp、browsers、shell、webview2）

## 当前代码结构

- `rust/src/`：
  - `http.rs` / `service.rs`：HTTP 端点分发（axum）
  - `control/`：页面操作（eval/click/fill...）、`screenshot.rs`（CDP 截图 + clip）、`accessibility.rs`（AX 树）、`responses.rs`（响应捕获）
  - `ui/`：`mod.rs`（UiState/布局/标签管理/面板开关/发送）、`annotate.rs`（批注/截图状态机）、`new_window.rs`（新窗口拦截）
  - `devices.rs`：14 个内置设备预设（`DEVICES`：desktop×3 / iPad×3 / iPhone×5 / Pixel×2 / galaxy-s23）+ UA 设置；另有不进表的面板首项「默认」`__default__`
- `frontend/src/`：`index.ts`（面板）、`overlay.ts`（覆盖层）、`toolbar.ts`（工具栏）、`page-bridge.ts`（页面桥）、`newtab.ts`、`theme.ts`（主题）、`types.ts`
- `src/`：`index.ts`（插件：注册 **2 个工具** `bt_cli` + `bt_set_media_mode`，`bt_cli` 内含 `COMMANDS` 子命令表〔当前 47 条〕、批注 drain 投递 `setupAnnotateDelivery`/`drainAnnotate`）、`client.ts`（HTTP 客户端 + 实例/令牌/会话管理）、`notify-server.ts`（`/notify/<sid>` 监听）、`binary.ts`（二进制下载 + 锁）、`config/`、`i18n/`、`logger.ts`、`util/sanitize.ts`
