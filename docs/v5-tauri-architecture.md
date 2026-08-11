# V5 架构：Tauri 2 跨平台多 Webview（方案变更说明）

> 变更原因：V4 方案（WebView2 + DirectComposition + Win32）完全绑定 Windows。
> 用户决策：直接换跨平台框架，确认走 **Tauri 2 多 Webview** 路线。
> 覆盖层方案：透明 Webview 叠加（非平台原生合成）。

## 目标形态（单窗口 + 多 Webview）

```
┌── Tauri 窗口(单窗口) ──────────────────────────┐
│  ├─ 页面 Webview(渲染目标网页,零注入)           │
│  ├─ 覆盖层 Webview(透明叠加,自绘高亮/批注标记)  │
│  └─ 面板 Webview(右侧并排,HTML 面板)            │
└─────────────────────────────────────────────────┘
        ↑ Rust 后端: eval 控制协议 + 事件系统
opencode 插件(client.ts) ← HTTP → Rust 服务(axum)
```

- **单窗口多 Webview**：Tauri 2 `WebviewBuilder` + `window.add_child()`
- **页面零注入**：页面 Webview 加载目标网页，控制通过 `eval_with_callback`（执行一次性 JS 查询/操作，不留驻脚本）
- **覆盖层**：透明 Webview（`transparent(true)`），Canvas 自绘高亮框/批注标记，叠加在页面 Webview 上
- **面板**：普通 Webview，HTML/CSS/JS 实现（迁移 V3 `src/panel/` 代码）

## 平台能力对照

| 能力 | Windows | Linux | macOS |
|------|---------|-------|-------|
| Webview 引擎 | WebView2 | WebKitGTK | WKWebView |
| 多 Webview | ✓ add_child | ✓ | ✓ |
| 透明 Webview | ✓ | ✓ | ✓(私有 API) |
| eval 控制 | ✓ | ✓ | ✓ |
| 底层访问 | `with_webview` → ICoreWebView2Controller | → WebKitWebView | → WKWebView |
| 截图 | CDP Page.captureScreenshot | WebKitGTK snapshot | WKWebView snapshot |

## 控制协议（统一 eval 层）

- `eval(js) -> Result<String>`：执行 JS，返回 JSON 序列化结果（跨平台一致）
- 页面操作全部通过 eval 完成：click/fill/select/hover/scroll/elementFromPoint/selector 生成
- 截图：平台分支（Windows CDP / Linux WebKitGTK snapshot）
- 导航事件：`on_navigation` / `on_page_load` / `on_new_window` 回调
- 网络/控制台：`on_web_resource_request`（拦截）+ eval 轮询（console）

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

## 实施要点

- 前端 TS 工程化：`frontend/src/*.ts`（index 面板 / overlay 覆盖层 / page-bridge 页面桥）+ esbuild 打包到 `frontend/dist`，HTML 模板与 TS 分离
- 控制协议关键点：`eval_with_callback` 会把 JS 返回值 JSON 序列化，JS 表达式直接返回对象，不要再包 JSON.stringify（否则双重编码）
- 覆盖层：透明 Webview（`transparent(true)`），批注模式 show（拦截鼠标）+ 普通模式 hide（页面交互），鼠标事件 emit `overlay-input` → Rust 状态机
- 页面桥：`initialization_script` 注入 `window.__pwPage`（query/state），导航后仍生效
- 批注记录：Rust `UiState.records` 持有，`records-changed` 事件推送面板
- HTTP 端口：client.ts 显式传 `--port`（release 模式 GUI 无控制台，不能解析 stdout），轮询就绪
- 截图（Windows）：`with_webview` 拿 ICoreWebView2Controller → CoreWebView2() → CDP Page.captureScreenshot；必须与 tauri 使用相同 webview2-com 版本（0.38.2 / windows 0.61.3），否则类型不兼容；completed 回调签名 `(windows::core::Result<()>, String)` → `windows::core::Result<()>`
- 发送全部：面板按钮 → invoke panel_cmd(send-all) → 记录快照入 `sent_records` 队列 → 插件端 startAnnotatePoller 每 2s 轮询 consume-sent → session.prompt 推送对话
- 清理：删除 Node 过渡代码（src/browsers、src/cdp、src/service、src/panel、src/bridge）、Rust 旧模块（cdp、browsers、shell、webview2）

## 保留资产

- `rust/src/cdp/`、`rust/src/browsers/`：协议层保留（eval 作为跨平台主协议，CDP 作 Windows 增强）
- `rust/src/service.rs`、`rust/src/http.rs`：HTTP 端点分发不变，浏览器管理替换为 Tauri Webview 管理
- `src/client.ts`：spawn 目标从 pw-shell 二进制改为 tauri 产物，端口协议不变
- `src/panel/`：V3 面板代码迁移到面板 Webview
- `src/bridge/http-server.ts`：记录管理逻辑保留（面板 Webview 内 fetch 改为 invoke 或保留 HTTP）
