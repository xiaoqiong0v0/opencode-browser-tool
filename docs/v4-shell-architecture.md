# V4 架构：外壳 + 原始调试浏览器（方案变更说明）

> 变更原因：现有注入式面板无法批注弹框类元素（点击/失焦会关闭弹框），注入方向根本行不通。
> 方案确认：外壳窗口 + 原始调试浏览器（CDP 直接控制），单窗口内嵌，页面零注入。

## 目标形态（单窗口内嵌）

```
┌── 外壳窗口(原生 UI) ─────────────────────────┐
│  ├─ 工具栏/编辑框/列表(原生 UI)               │
│  ├─ 覆盖渲染层(透明窗口,画高亮/批注标记)       │
│  └─ 嵌入式浏览器区(SetParent 挂载浏览器窗口)   │
└───────────────────────────────────────────────┘
        ↑ CDP 控制
Shell 服务进程(Node) ← HTTP ← opencode 插件
```

- 浏览器进程以 `--app=URL` 启动：窗口仅含页面，无标签栏/地址栏
- `SetParent`(Windows) / `XReparentWindow`(Linux) 嵌入外壳
- 覆盖层通过 CDP `DOM.getBoxModel` 获取元素坐标 → 自绘高亮框/批注标记
- 页面零注入：弹框类元素可正常批注，点击/失焦不影响

## 浏览器控制：原始调试协议（弃用 Playwright 运行时）

| 项 | 方案 |
|----|------|
| 启动 | `spawn(chrome.exe, [--app=<url>, --remote-debugging-port=<port>, ...])` |
| 控制协议 | CDP（WebSocket 直连，自封装客户端） |
| 浏览器二进制 | 固定版本号，构建时生成清单，运行时 lazy 下载 |
| Firefox | V2 接入 WebDriver BiDi（接口预留） |

### 浏览器安装（摆脱 playwright install 命令）

- **版本定死**：manifest 中固定 chromium 版本（如 chromium-1228）
- **开发时**：用 Playwright 获取文件列表（后缀固定），生成静态清单 `src/browsers/manifest.ts`
- **运行时**：首次使用时检查本地 `browsersPath/<name>-<revision>/`，不存在则根据镜像配置下载
  - 国内镜像：npmmirror（默认，与现有 `playwrightMirror` 配置一致）
  - 海外源：playwright CDN（可配置）
- **懒加载**：用到才下载，下载完成写 `INSTALLATION_COMPLETE` 标记

## 里程碑

| 阶段 | 内容 | 验收 |
|------|------|------|
| M1 | CDP 客户端 + `--app` 启动 + lazy 下载器 | spawn 浏览器、CDP 导航/截图 |
| M2 | Shell 服务迁移现有 `/api/*` 命令为 CDP | pw_* 工具回归通过 |
| M3 | 外壳窗口 + 覆盖层 + 批注改造 | 弹框场景批注不关闭 |
| M4 | 面板功能迁移（截图/列表/发送）、i18n、缓存 | 现有功能回归 |
| M5 | Firefox BiDi、Linux 验证、文档 | 跨平台可用 |

## 实施状态

### M1 已完成（Rust 实现）

- `rust/src/cdp/client.rs` — 原始 CDP WebSocket 客户端（tokio + tokio-tungstenite，请求/响应 + 事件订阅）
- `rust/src/cdp/browser.rs` — `--app` 模式启动 + 页面 target 发现 + navigate/evaluate/getBoxModel/screenshot
- 已验证：chromium 以 `--app` 模式启动、CDP 连接、导航、文本提取、元素坐标、截图全通过
- Node 版 `src/cdp/`、`src/browsers/` 为过渡实现，待 M2 完成后删除

### M2 已完成（Rust 服务 + Node 桥接）

- `rust/src/cdp/actions.rs` — 交互操作（click/fill/select/hover/press-key/drag/upload-file/wait-for-selector/scroll/scroll-to-element/element-state/dropdown-options/visible-html/resize/user-agent/pdf）
- `rust/src/cdp/session.rs` — 会话状态（console 日志、网络响应监听）+ EventHub 事件分发
- `rust/src/service.rs` — 40 个 `/api/*` 端点分发（响应格式与旧 Node 服务兼容 `{success, data}`）
- `rust/src/http.rs` — axum HTTP 服务（端口写 stdout，与 client.ts 协议一致）
- `rust/src/browsers/` — 清单 + lazy 下载器（镜像规则支持 npmmirror）
- `src/client.ts` — `startService` 改为 spawn Rust 二进制（自动发现: dist/bin → rust/target）
- 已验证：HTTP 服务端到端（navigate/status/evaluate/visible-text/element-state/screenshot/console-logs/click/tabs/close 全通过）

### M3 进行中（外壳窗口 + 覆盖层）

- `rust/src/shell/host.rs` — Win32 宿主窗口（WS_OVERLAPPEDWINDOW + WM_SIZE 自动布局子窗口）
- `rust/src/shell/overlay.rs` — 透明点击穿透覆盖层（WS_EX_LAYERED|TRANSPARENT|NOACTIVATE，GWLP_USERDATA 存高亮数据，WM_PAINT 绘制）
- 已验证：外壳窗口创建、浏览器 `--app` 窗口按 PID 枚举 + `SetParent` 嵌入、覆盖层 3 色高亮绘制
- 待办：批注交互（鼠标坐标 → elementFromPoint → selector 生成）、覆盖层数据与 CDP 联动、面板 UI 迁移

### 工具链（Windows）

- Rust stable-x86_64-pc-windows-msvc 1.97.1（rustup 管理）
- VS Build Tools 2022 + VCTools（MSVC 14.44，通过图形安装器补装）
- GNU/gnullvm 工具链已卸载清理

## 与原架构差异

- `src/service/index.ts`：Playwright 实现 → CDP 客户端实现
- `src/panel/`：页面注入 → 外壳原生 UI（或外壳内嵌 HTML UI）
- 新增 `src/cdp/`（协议客户端）、`src/shell/`（外壳进程）、`src/browsers/`（清单+下载器）
- `src/client.ts`、`src/index.ts` 工具定义层基本不变
- 依赖：移除运行时 `playwright`（仅 dev 用于生成清单）
