# 实施状态

## 当前架构（V5：跨平台 Tauri 2 多 Webview）

单窗口 + 三 Webview（页面/覆盖层/面板），前端 TS 工程化，详见 [v5-tauri-architecture.md](./v5-tauri-architecture.md)。

### 已完成
- [x] V5.1 Tauri 骨架：单窗口 + 三 Webview（页面/覆盖层/面板），页面桥注入
- [x] V5.2 控制协议：eval 统一封装（导航/点击/填写/文本/状态/滚动/等待/查询）
- [x] V5.3 覆盖层批注：透明 Webview + Canvas，悬停高亮/点击标记/右键退出
- [x] V5.4 面板：TS 前端（列表/按钮/说明输入），Rust 命令接通，发送队列+插件端轮询推送
- [x] V5.5 HTTP 服务对接 + 插件集成，全链路端点回归通过
- [x] V5.6 截图 Windows CDP 实现（PNG base64 验证通过）

### 关键经验
- `eval_with_callback` 自动 JSON 序列化 JS 返回值，表达式直接返回对象（勿双重 stringify）
- release 模式 Tauri GUI 无控制台，HTTP 端口需 `--port` 显式指定 + 轮询就绪
- 覆盖层 show/hide 控制鼠标拦截（批注模式拦截，普通模式穿透）
- Windows 必须显式 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，否则窗口被虚拟化缩放与 WebView2 布局错位
- 截图平台分支：Windows CDP（webview2-com 版本须与 tauri 对齐 0.38.2）
- 调试注意：非 DPI 感知进程读取 GetWindowRect 得到虚拟化坐标，截图会错位

## 待办

- 截图平台分支（Linux WebKitGTK snapshot / macOS）
- 面板 i18n 完整迁移
- 测试目录重写（V5 时代）
- Linux/macOS 验证
