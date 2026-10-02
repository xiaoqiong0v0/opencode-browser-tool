# Kali(Linux)可信输入复验清单

> 目的：在**真实 Linux 桌面**（GNOME/KDE，Wayland 或 X11）上复验 Linux 侧的可信输入与降级策略。
> 依据与 WSLg 取证见 `.tmp/scratch/linux-gdk-evidence.md`；平台能力与降级规则见 `linux-support.md` §7。
> 所有命令可直接照抄，只有 `<PORT>`、路径与发行版名需按环境替换。

## 1. 前置依赖

系统依赖（Debian 系）：

```bash
sudo apt update && sudo apt install -y \
  libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev \
  libxdo-dev libssl-dev build-essential cmake clang libclang-dev
```

Rust 工具链与固定构建目录（避免污染默认 target）：

```bash
rustc --version && cargo --version
export CARGO_TARGET_DIR=/home/afubek/bt-target     # 可按需改
```

## 2. 构建

```bash
cd /mnt/d/WorkSpace/my_open/opencode-browser-tool/rust   # 或你 clone 的实际路径
CARGO_TARGET_DIR=$CARGO_TARGET_DIR cargo build --release
# 期望：Finished，0 warning 0 error；产物 $CARGO_TARGET_DIR/release/bt-shell
```

> 注意：若已有正在运行的 bt-shell 占用 `rust/target/release/bt-shell.exe`，构建会报
> `failed to remove file ... (os error 5)`——关闭该实例后重跑即可（本仓库 Windows 侧遇过）。

## 3. 启动 shell

在带图形会话的终端里（需能访问 `$WAYLAND_DISPLAY` 或 `$DISPLAY`）：

```bash
cd /mnt/d/WorkSpace/my_open/opencode-browser-tool
$CARGO_TARGET_DIR/release/bt-shell --port 8790 --user-data-dir /tmp/bt-verify-ud \
  > .tmp/scratch/verify.log 2>&1 &
```

等就绪（最多几秒）：

```bash
for i in $(seq 1 20); do
  curl -sf -X POST http://127.0.0.1:8790/api/status -H 'Content-Type: application/json' -d '{}' && break
  sleep 0.5
done
```

## 4. HTTP 驱动命令（两种写法）

服务只监听 `127.0.0.1`：WSL 内用 `curl`；Windows 侧经 WSL2 localhost 转发用 `Invoke-RestMethod`。

**curl（Linux 内）**

```bash
API=http://127.0.0.1:8790/api
post() { curl -s -X POST "$API/$1" -H 'Content-Type: application/json' -d "$2"; }

# 打开测试页（路径按实际仓库位置）
post navigate '{"url":"file:///mnt/d/WorkSpace/my_open/opencode-browser-tool/tests/manual/input-test.html"}'
# 清空事件缓冲
post evaluate '{"script":"(window.__events.length=0, true)"}'
# 取判据
post evaluate '{"script":"JSON.stringify({n:__events.length,trusted:__events.filter(function(e){return e.isTrusted}).length,value:document.querySelector(\"#text\").value,submit:__events.filter(function(e){return e.type===\"submit\"}).length})"}'
```

**Invoke-RestMethod（Windows 侧，PowerShell）**

```powershell
$API = "http://127.0.0.1:8790/api"
function Post($ep, $obj) {
  Invoke-RestMethod -Uri "$API/$ep" -Method Post -ContentType 'application/json' `
    -Body ($obj | ConvertTo-Json) -TimeoutSec 15
}
Post navigate @{ url = "file:///mnt/d/WorkSpace/my_open/opencode-browser-tool/tests/manual/input-test.html" }
Post evaluate @{ script = "(window.__events.length=0, true)" }
Post evaluate @{ script = "JSON.stringify({n:__events.length,trusted:__events.filter(function(e){return e.isTrusted}).length,value:document.querySelector('#text').value,submit:__events.filter(function(e){return e.type==='submit'}).length})" }
```

## 5. 逐项检查清单与预期结果

测试页：`tests/manual/input-test.html`（含受控组件 `#controlled`、遮挡用例 `#covered-btn`、hover 目标、
同源 iframe `#ifr`、文件输入 `#file`、拖拽 `#drag-src`→`#drop-zone`）。

| # | 命令（API） | 预期结果 |
|---|---|---|
| 1 | `press-key {"key":"a","selector":"#text"}` | 成功；`evaluate` 读 `#text.value==="a"`，且 `__events` 中按键事件 `isTrusted` 全为 `true` |
| 2 | 清空事件后 `press-key {"key":"Enter","selector":"#text"}` | 成功；`__events` 出现 `submit`，`trusted` 计数 == 事件总数 |
| 3 | `press-key {"key":"Tab","selector":"#text"}` | `document.activeElement` 变为下一个可聚焦元素（如提交按钮） |
| 4 | `click {"selector":"#mb"}` | 成功；`window.__mouse` 的 `down/up/click` 各 +1，`last.isTrusted===true`，坐标与元素中心一致 |
| 5 | `click {"selector":"#covered-btn"}` | **明确报错** `element is covered at its center ... (hit: #cover-overlay)`，`window.__covered.clicks===0` |
| 6 | `hover {"selector":"#hover-target"}` | 成功；`el.matches(':hover')===true`，背景色变为 `rgb(43, 138, 62)` |
| 7 | `fill {"selector":"#text","value":"Hello 1!"}` | 成功；`#text.value==="Hello 1!"`，且 `beforeinput`/`input` 均 `isTrusted===true` |
| 8 | `fill {"selector":"#controlled","value":"xyz"}` | 成功；`window.__controlled.state==="xyz"`、`trustedInputs` 递增（受控组件接受可信输入） |
| 9 | `select {"selector":"#sel","value":"o3"}` | 成功且回传 `value:"o3"`；`#sel.value==="o3"`，出现可信 `input`/`change` |
| 10 | 对 `multiple`/`size>1` 的 select 执行 `select` | 返回带 `degraded:true` 与 `degradedReason`（`untrusted JS fallback ...`）——**这是预期降级**，非失败 |
| 11 | `iframe-click {"iframeSelector":"#ifr","selector":"#ifr-btn"}` | 成功；`window.__iframeClicks===1`（坐标含 iframe 偏移换算） |
| 12 | `iframe-fill {"iframeSelector":"#ifr","selector":"#ifr-text","value":"abc"}` | 成功；iframe 内 `#ifr-text.value==="abc"` |
| 13 | `upload-file {"selector":"#file","filePath":"/etc/hostname"}` | **Linux 上预期报错**：`upload_file: not implemented on this platform yet (Windows via CDP ...)`。不返回成功即正确 |
| 14 | `drag {"sourceSelector":"#drag-src","targetSelector":"#drop-zone"}` | **Linux 上预期报错**：`drag: not implemented on this platform yet (...)`。不返回成功即正确 |
| 15 | `scroll {"direction":"down","amount":600}` | 成功；`stable:true`；返回后立刻与 250ms 后读 `window.scrollY` 相同（已稳定） |
| 16 | `scroll-to-element {"selector":"#dump"}` 与 `{"selector":"#sc-bottom"}`（**内层滚动容器**内） | 均成功；`reached:true`/`inViewport:true`；元素 `getBoundingClientRect()` 与视口相交（`#sc-bottom` 需内层容器与窗口同时滚动到位） |
| 17 | `navigate` 到 `tests/manual/slow-page.html?block=1200` | 成功且 `ready:true`、`navigated:true`；**返回时 `location.href` 已是目标页**、`#slow-ready` 已存在、`readyState` 为 `interactive`/`complete`（就绪判定要求**文档身份/URL 已变化**，不会把旧页的 `complete` 误判为就绪） |

超时/降级的有界行为（可选，用环境变量调小超时，起实例时带 `BT_NAV_TIMEOUT_MS=1500 BT_SCROLL_TIMEOUT_MS=800`）：

- `navigate` 到 `tests/manual/never-ready.html` → ~1.5s 返回，`ready:false, navigated:true, timedOut:true, readyState:"loading"`（**如实报告，不假装成功**；`navigated` 表示文档已切换但未加载完）；
- `navigate` 到 `tests/manual/slow-page.html?autoscroll=1&block=0` 后 `scroll` → ~0.8s 返回，`stable:false, timedOut:true`。

> 就绪判定依据：**文档身份变化**（`performance.timeOrigin`，覆盖新文档加载与**同 URL 重载**）或 **URL 变化**（覆盖 SPA `pushState` 与 bfcache 前进/后退）。`go_back`/`go_forward` 共用该判定。

目标**物理不可达**时（例如 `position:fixed` 且位于视口外的元素，滚动无法改变其位置）：`scroll_to_element`
仍会在有界时间内返回 `reached:false, inViewport:false, timedOut:true`，并附带
`reason:"element not brought into view (possibly outside the document's scrollable range)"`——**如实说明原因，不假装成功**。

## 6. 失败时如何取证

1. 服务日志：启动时已重定向到 `.tmp/scratch/verify.log`；另可用
   `console_logs` 端点读取页面 `console.*`：
   `post console-logs '{"type":"all","limit":100}'`（或 PowerShell `Post console-logs @{ type="all"; limit=100 }`）。
2. 逐条记录 `evaluate` 原始返回（`window.__events` / `__mouse` / `__hover` / `__controlled` / `__iframeClicks`）。
3. 保留构建与运行输出：`cargo build --release 2>&1 | tee .tmp/scratch/build.log`、上文的 `verify.log`。
4. 若报“可信注入无效”，对比本文 §5 的 `isTrusted` 判据；`degraded:true` 表示走了降级（见 `linux-support.md` §7）。
