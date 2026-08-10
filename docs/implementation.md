# 实施状态

## 已完成

### V1：Worker 架构（已废弃）
- BrowserManager + chrome-worker 子进程 + 41 个独立工具文件
- Proxy page 对象 + IPC 通信 + eval 序列化 hack
- 问题：proxy 无法传递 ElementHandle/BrowserContext/Keyboard 等子对象，每个都需要加专用命令

### V2：HTTP 服务架构（当前）
- 移除 BrowserManager、chrome-worker、41 个工具文件
- Node HTTP 服务 + 插件端 `client.ts`，所有操作通过 `POST /api/{command}` 通信
- 工具定义全部内联在 `index.ts`，每个工具一个对象
- 序列化统一：服务端 `JSON.stringify` → IPC → 客户端 `JSON.parse`

#### 核心功能 ✓
- [x] 浏览器启动 / 关闭 / 状态查询
- [x] 导航、前进后退、刷新
- [x] 点击、填写、清空、选择、悬停、拖拽、按键、上传文件
- [x] 截图（全页/元素）、evaluate、可见文本、HTML 结构
- [x] 控制台日志、元素状态、下拉框选项
- [x] 滚动、滚动到元素、等待选择器
- [x] 标签页管理（新建/切换/关闭/列表/点击切换）
- [x] iframe 操作（点击/填写）
- [x] 自定义 User-Agent、设备预设选择
- [x] 保存 PDF、显示通知
- [x] expect_response / assert_response
- [x] 多浏览器支持（chromium / firefox / webkit，工具调用时动态切换）
- [x] 会话隔离（可选，默认关闭）
- [x] Node.js 检测（支持自定义路径）
- [x] 多语言（中/英切换，工具描述 + 输出消息）

#### 已知问题
- `pw_get_accessibility_tree` — Playwright 1.61.1 已移除 `page.accessibility`

### V3（当前）
- 面板全 Shadow DOM 隔离，不受页面 CSS 影响
- `all:initial` 移除，改用 `!important` 强制关键属性
- 右键/双击取消批注模式 + 通知提示
- 安装流程统一到 `getOrCreatePage`，异步 `spawn` 不阻塞
- 浏览器下载进度实时打印到日志
- `composedPath()` 检测面板事件来源，兼容 Shadow DOM

### V3.1
- 修复 jsonc 配置解析：剥离注释后存在尾逗号导致 `JSON.parse` 失败、配置静默失效（如 `panelLang` 不生效）
- 解析时容错处理尾逗号（`.replace(/,\s*}/g, "}")`）

## 待办

（无，所有功能已验证通过）
