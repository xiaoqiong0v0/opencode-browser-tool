# opencode-playwright-tool 架构设计

> 当前状态：V3 注入式架构（见下方）。V4 外壳 + 原始调试浏览器方案已确认，见 [v4-shell-architecture.md](./v4-shell-architecture.md)，实施中。

## 概述

opencode-playwright-tool 是一个 opencode 插件，通过独立 Node.js 子进程管理 Playwright 浏览器。插件和浏览器进程通过 HTTP 通信，彻底消除 Bun × Playwright 不兼容问题和 proxy 对象链限制。

## 核心架构

```
opencode（Bun）
└─ init() → spawn("node", ["service/index.js"])
     └─ Node HTTP 服务（127.0.0.1:PORT）
          └─ Playwright → Chromium / Firefox / WebKit
```

### 启动流程

1. 插件 `init()` → 调用 `startService()`
2. `spawn("node", ["dist/service/index.js"])`
3. 服务监听 `127.0.0.1:0`（系统分配端口），端口号写入 stdout
4. 插件读取端口号 → `serviceReady = true`
5. 后续所有工具调用发送 `POST http://127.0.0.1:{PORT}/api/{command}`

### 工具调用流程

```
LLM 决定调用工具
  → opencode 执行插件 tool.execute()
    → _exec 注入 _sessionId
      → client.callApi("/api/xxx", params)
        → fetch("POST http://127.0.0.1:{PORT}/api/xxx")
          → service handler → Playwright API
            → 返回 JSON { success, data }
```

## 项目结构

```
src/
├── index.ts              # 插件入口：工具定义 + 服务启动
├── client.ts             # HTTP 客户端：startService + service.* API
├── config/
│   └── index.ts          # 配置加载（jsonc + env）
├── i18n/
│   ├── index.ts          # 多语言支持（t() / panelT()）
│   ├── en.ts             # 英文语言包
│   └── zh.ts             # 中文语言包
├── service/
│   └── index.ts          # Node HTTP 服务：路由 + Playwright 管理
├── panel/
│   ├── config.ts         # 面板配置
│   ├── state.ts          # 面板状态
│   ├── dom.ts            # DOM 工具
│   ├── core.ts           # 共享逻辑
│   ├── annotate.ts       # 批注模式
│   ├── shot.ts           # 截图模式
│   ├── handlers.ts       # 事件处理
│   └── panel.ts          # esbuild 入口
├── bridge/
│   └── http-server.ts    # HTTP Bridge（面板记录管理）
└── injects/              # 注入脚本（已弃用，保留历史）
```

## Node 服务（`service/index.ts`）

### 路由处理

所有请求通过 `handler(url, body)` 统一处理。URL 映射到 Playwright 操作：

```
POST /api/navigate      → page.goto(url)
POST /api/click          → page.click(selector)
POST /api/evaluate       → page.evaluate(script)
POST /api/screenshot     → page.screenshot() / elementHandle.screenshot()
...（30+ 端点）
```

### 浏览器管理

- `getOrCreatePage(sessionId?)` — 惰性启动浏览器，返回当前活动页
- 浏览器断开时自动清理状态
- 支持 `--session-isolation` 标志，按 sessionId 管理独立 BrowserContext

### 多浏览器支持

```ts
const browserName = process.argv.find(a => a.startsWith("--browser=")) || "chromium";
const pw = await import("playwright");
const launcher = pw[browserName];
browser = await launcher.launch();
```

`pw_navigate` 可带 `browserType` 参数动态切换浏览器引擎。

## 会话管理

### 非隔离模式（默认）

所有 session 共享 `activePage`，父子会话同用一个浏览器页面。

### 隔离模式（`sessionIsolation: true`）

```ts
const sessions = Map<sessionId, { context: BrowserContext, page: Page }>
getOrCreatePage(sid):
  if (sessionIsolation && sid)
    return sessions.get(sid)?.page ?? create new BrowserContext
  else
    return activePage
```

## HTTP 客户端（`client.ts`）

- `startService()` — spawn Node 子进程，等待端口就绪
- `stopService()` — 发送 SIGTERM 终止服务
- `callApi(path, body, sessionId)` — 发送 POST 请求，包装响应
- `service.xxx(params)` — 每个 API 端点对应一个方法

### _sessionId 注入

```ts
_exec = (fn) => async (a, ctx?) =>
  fn({ ...a, _sessionId: ctx?.sessionID || "" })
```

## 关键决策

| 决策 | 选择 | 原因 |
|------|------|------|
| 浏览器运行时 | 独立 Node 子进程 | Bun × Playwright CDP/WebSocket 不兼容 |
| 通信协议 | HTTP（POST JSON） | 清晰可调试，curl 可直接测试 |
| 工具定义 | 内联在 index.ts | 去掉 41 个工具文件，减少 proxy/序列化 hack |
| 面板通信 | HTTP Bridge | 面板 JS 和插件解耦 |
| 会话隔离 | 可选，默认关闭 | 多数场景父子会话共享浏览器更合理 |
| 浏览器切换 | navigate 参数动态切换 | 由 LLM 按需决定，不写死在配置 |

## 配置

```jsonc
{
  "panelLang": "en",         // 面板语言
  "toolLang": "en",           // 工具描述语言
  "sessionIsolation": false,  // 会话隔离
  "browserType": "chromium",  // 默认浏览器类型
  "disabledTools": [],        // 禁用工具列表
  "browsersPath": "...",      // 浏览器缓存路径
  "nodePath": "...",          // Node.js 路径
}
```

环境变量（优先级最高）：`PW_LANG`, `PW_BROWSERS_PATH`, `PW_NODE_PATH`
