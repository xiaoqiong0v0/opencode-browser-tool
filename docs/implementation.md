# 实施状态

## 当前架构（V5：跨平台 Tauri 2 多 Webview）

单窗口 + 多 Webview（toolbar / page-1..N 真多标签 / overlay / panel / newtab），前端 TS 工程化，详见 [v5-tauri-architecture.md](./v5-tauri-architecture.md)。

### 已完成
- [x] V5.1 Tauri 骨架：单窗口 + 三 Webview（页面/覆盖层/面板），页面桥注入
- [x] V5.2 控制协议：eval 统一封装（导航/点击/填写/文本/状态/滚动/等待/查询）
- [x] V5.3 覆盖层批注：透明 Webview + Canvas，悬停高亮/点击标记/右键退出
- [x] V5.4 面板：TS 前端（列表/按钮/说明输入），Rust 命令接通，发送队列+插件端轮询推送
- [x] V5.5 HTTP 服务对接 + 插件集成，全链路端点回归通过
- [x] V5.6 截图 Windows CDP 实现（PNG base64 验证通过，支持 clip 区域截图）
- [x] V5.7 窗口 UI：无标题栏 + 自定义窗口按钮、主题系统、真多标签（每标签独立 webview）、newtab、事件驱动状态同步
- [x] V5.8 批注/截图双模式：遮罩式选区裁剪、保存需说明、发送后清空、记录卡片化、面板 toast
- [x] V5.9 真多标签接 HTTP + 新窗口拦截（target=_blank → 新标签）+ z-order 置顶
- [x] V5.10 工具补齐：expect/assert 网络响应捕获、可访问性树、插件取字段修复
- [x] 插件命令与 Rust 端点已全部对齐，无不可用项
- [x] 依赖清理：移除 adm-zip/esbuild/tsx 旧 Playwright 残留与 test/ 目录；lucide 移 devDependencies
- [x] 截图 Linux 分支：WebKitGTK `WebView::snapshot`（Visible 区域）+ cairo 裁剪 → PNG base64

### 关键经验
- `eval_with_callback` 自动 JSON 序列化 JS 返回值，表达式直接返回对象（勿双重 stringify）
- **opencode 宿主对 tool 返回值要求字符串**：命令 `run` 返回对象会让宿主（Bun/JSC）报 `undefined is not an object (evaluating 'c.split')`；结构化结果请 `JSON.stringify` 或自行格式化（截图类返回 image attachment 属例外）
- **Windows 可访问性树（CDP）**：`Accessibility.getPartialAXTree` 即使 `fetchRelatives=false` 也只返回命中节点自身（其 `parentId` 指向不在结果内的祖先）→ 取"某选择器子树"应改用 `DOM.getDocument`+`DOM.querySelector`+`DOM.describeNode`(取 backendNodeId) 定位命中 AX 节点，再用 `Accessibility.getFullAXTree` 并以该节点为根建树；CDP 会把文本拆成逐字符 `StaticText` + `InlineTextBox`，且 `html`/`body`/无名容器为 ignored → 需丢弃 InlineTextBox、把 StaticText 文本合并进父节点 name、并按"name 值为空"跳过无名 generic（原判据 `name` 字段缺失对 CDP 恒 false）
- release 模式 Tauri GUI 无控制台，HTTP 端口需 `--port` 显式指定 + 轮询就绪
- 覆盖层 show/hide 控制鼠标拦截（批注模式拦截，普通模式穿透）
- Windows 必须显式 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，否则窗口被虚拟化缩放与 WebView2 布局错位
- 截图平台分支：Windows CDP（webview2-com 版本须与 tauri 对齐 0.38.2）；Linux WebKitGTK `WebView::snapshot` 只有 `Visible`/`FullDocument` 两种区域、**无矩形参数**，clip 需先截 Visible 再用 cairo 偏移裁剪（`set_source_surface` + `paint`）；`Surface::write_to_png` 位于 cairo-rs 的 `png` feature 下而 gtk 0.18 未启用，故 Linux target 需显式加 `cairo-rs = { version = "0.18", features = ["png"] }`；截图调用方都在后台线程，`with_webview` 在非主线程仅投递消息到 GTK 主线程，主线程阻塞 `recv_timeout` 会死锁、后台线程不会
- 动态创建的 page webview 会盖在 overlay/panel 之上 → Windows SetWindowPos(HWND_TOP) 置顶
- 页面 file:// 的 tauri IPC emit 被拒（Origin 非法）→ 新窗口拦截用 WebView2 原生事件而非页面 JS
- AX 树解析：过滤 generic 容器自身但透传子树，否则只剩根节点
- Mutex 加锁注意提前 drop，避免同一线程重入死锁（emit_mode_state 曾卡死 HTTP 服务）
- 前端 build 的 copyHtml 可能不更新 dist HTML（esbuild 缓存）→ 需手动 Copy-Item src/*.html dist/
- 调试注意：非 DPI 感知进程读取 GetWindowRect 得到虚拟化坐标，截图会错位

## 待办

- 截图平台分支（macOS WKWebView snapshot）
- 新窗口拦截 / 响应捕获 / 可访问性树 的 Linux/macOS 分支
- Linux/macOS 实机验证
