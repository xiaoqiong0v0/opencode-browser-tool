#!/usr/bin/env bash
# 一键 Linux(WSLg)复验：可选构建 -> 起实例 -> 跑 15 项 -> PASS/FAIL 报告 -> 关闭自起实例
#
# 用法: bash tests/manual/verify-linux.sh [--port N] [--no-build] [--keep]
#   --port N     默认 8790；端口被占用则报错退出
#   --no-build   跳过构建（直接用已探测到的二进制）
#   --keep       跑完保留实例（打印 PID 供人工查看）
#
# 仅依赖 bash + curl（判据用页面返回的 "VERDICT ok/fail: ..." + grep，不解析外层 JSON、不用 jq）。
# 清理只针对本脚本记录的 PID（trap EXIT），绝不按进程名批量杀。

set -u

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
SCRATCH="$REPO/.tmp/scratch"
PORT=8790
DO_BUILD=1
KEEP=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2;;
    --no-build) DO_BUILD=0; shift;;
    --keep) KEEP=1; shift;;
    -h|--help) sed -n '2,9p' "$0"; exit 0;;
    *) echo "未知参数: $1"; exit 2;;
  esac
done

TS="$(date +%Y%m%d-%H%M%S)"
mkdir -p "$SCRATCH"
LOG="$SCRATCH/verify-linux-$TS.log"
APP_LOG="$SCRATCH/verify-linux-$TS.app.log"

PASS=0
FAIL=0
FAILED=""
PID=""

log() { echo "$*" | tee -a "$LOG"; }

cleanup() {
  if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
    if [ "$KEEP" = "1" ]; then
      log "[keep] 实例保留运行: PID $PID (port $PORT)"
    else
      kill -TERM "$PID" 2>/dev/null
      sleep 1
      if kill -0 "$PID" 2>/dev/null; then kill -9 "$PID" 2>/dev/null; fi
      log "[cleanup] 已停止自起实例 PID $PID"
    fi
  fi
}
trap cleanup EXIT INT TERM

# ---- 报告辅助 ----
report() { # report <num> <name> <0|1> <detail>
  if [ "$3" = "1" ]; then
    log "PASS [$1] $2 :: $4"; PASS=$((PASS + 1))
  else
    log "FAIL [$1] $2 :: $4"; FAIL=$((FAIL + 1)); FAILED="$FAILED $1"
  fi
}

# 从 evaluate 响应里取 "VERDICT ..." 文本并判 PASS/FAIL
case_from_verdict() { # case_from_verdict <num> <name> <response>
  local num="$1" name="$2" resp="$3" line
  line="$(printf '%s' "$resp" | grep -o 'VERDICT [^"]*' | head -1)"
  case "$line" in
    "VERDICT ok"*) report "$num" "$name" 1 "$line";;
    *) report "$num" "$name" 0 "${line:-<no verdict>} | raw=$(printf '%s' "$resp" | head -c 240)";;
  esac
}

# ---- JSON / API 辅助（不依赖 jq）----
json_escape() { local s="$1"; s="${s//\\/\\\\}"; s="${s//\"/\\\"}"; printf '%s' "$s"; }
api() { curl -s --max-time 25 -X POST "http://127.0.0.1:$PORT/api/$1" -H 'Content-Type: application/json' -d "$2"; }
evaljs() { local js="$1" esc; esc="$(json_escape "$js")"; api evaluate "{\"script\":\"$esc\"}"; }

# ---- 二进制探测 ----
BIN=""
if [ -n "${CARGO_TARGET_DIR:-}" ] && [ -x "$CARGO_TARGET_DIR/release/bt-shell" ]; then
  BIN="$CARGO_TARGET_DIR/release/bt-shell"
fi
if [ -z "$BIN" ] && [ -x "$REPO/rust/target/release/bt-shell" ]; then
  BIN="$REPO/rust/target/release/bt-shell"
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/rust/target}"
if [ "$DO_BUILD" = "1" ]; then
  log "[build] cd $REPO/rust && CARGO_TARGET_DIR=$TARGET_DIR cargo build --release"
  if ! (cd "$REPO/rust" && CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release >>"$LOG" 2>&1); then
    log "[build] 构建失败，见 $LOG"; exit 1
  fi
  BIN="$TARGET_DIR/release/bt-shell"
fi
if [ -z "$BIN" ] || [ ! -x "$BIN" ]; then
  log "[error] 未找到可执行文件（尝试: \${CARGO_TARGET_DIR}/release/bt-shell 或 $REPO/rust/target/release/bt-shell）"; exit 1
fi
log "[bin] $BIN"

# ---- 端口占用检查 ----
port_in_use() {
  if command -v ss >/dev/null 2>&1; then
    ss -ltn 2>/dev/null | grep -Eq ":$PORT([^0-9]|$)"
  else
    (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null
  fi
}
if port_in_use; then
  log "[error] 端口 $PORT 已被占用，换一个 --port"; exit 1
fi

# ---- 起实例 ----
"$BIN" --port "$PORT" --user-data-dir "/tmp/bt-verify-ud-$$" >"$APP_LOG" 2>&1 &
PID=$!
log "[start] $BIN --port $PORT (PID $PID)  日志: $APP_LOG"

READY=0
i=0
while [ "$i" -lt 40 ]; do
  if curl -s --max-time 2 -X POST "http://127.0.0.1:$PORT/api/status" -H 'Content-Type: application/json' -d '{}' | grep -q '"success":true'; then READY=1; break; fi
  sleep 0.5; i=$((i + 1))
done
if [ "$READY" != "1" ]; then log "[error] 实例未就绪（见 $APP_LOG）"; exit 1; fi
log "[ready] 实例就绪"

TESTPAGE="file://$REPO/tests/manual/input-test.html"
SLOWPAGE="file://$REPO/tests/manual/slow-page.html?block=1200"
log "==== verify-linux 15 项 ===="

# 打开测试页
api navigate "{\"url\":\"$(json_escape "$TESTPAGE")\"}" >/dev/null
sleep 0.5

# 1) 键盘 a
evaljs '(document.getElementById("text").value="", window.__events.length=0, true)' >/dev/null
api press-key '{"key":"a","selector":"#text"}' >/dev/null
sleep 0.4
case_from_verdict 1 "press-key a #text" "$(evaljs '(function(){var ks=window.__events.filter(function(x){return x.type==="keydown"||x.type==="keyup"||x.type==="keypress"||x.type==="input"||x.type==="beforeinput"});var n=ks.length;var t=ks.filter(function(x){return x.isTrusted}).length;var v=document.getElementById("text").value;return (v==="a"&&n>0&&t===n)?("VERDICT ok value="+v+" trusted="+t+"/"+n):("VERDICT fail value="+v+" trusted="+t+"/"+n);})()')"

# 2) 键盘 Enter
evaljs '(window.__events.length=0, true)' >/dev/null
api press-key '{"key":"Enter","selector":"#text"}' >/dev/null
sleep 0.4
case_from_verdict 2 "press-key Enter #text" "$(evaljs '(function(){var s=window.__events.filter(function(x){return x.type==="submit"}).length;return s>0?("VERDICT ok submit="+s):("VERDICT fail submit=0");})()')"

# 3) 键盘 Tab
evaljs '(window.__events.length=0, true)' >/dev/null
api press-key '{"key":"Tab","selector":"#text"}' >/dev/null
sleep 0.4
case_from_verdict 3 "press-key Tab #text" "$(evaljs '(function(){var a=document.activeElement;var id=(a&&a.id)?a.id:(a?a.tagName.toLowerCase():"null");return (id!=="text")?("VERDICT ok active="+id):("VERDICT fail active="+id);})()')"

# 4) 点击 #mb
evaljs '(window.__mouse.down=0,window.__mouse.up=0,window.__mouse.click=0,window.__mouse.last=null,true)' >/dev/null
api click '{"selector":"#mb"}' >/dev/null
sleep 0.4
case_from_verdict 4 "click #mb" "$(evaljs '(function(){var m=window.__mouse;var ok=(m.down>=1&&m.up>=1&&m.click>=1&&m.last&&m.last.isTrusted===true);return ok?("VERDICT ok down="+m.down+" up="+m.up+" click="+m.click+" trusted="+m.last.isTrusted):("VERDICT fail mouse="+JSON.stringify(m));})()')"

# 5) 被遮挡按钮（预期失败）
evaljs '(window.__covered.clicks=0, true)' >/dev/null
RESP5="$(api click '{"selector":"#covered-btn"}')"
CL5="$(evaljs 'window.__covered.clicks')"
if printf '%s' "$RESP5" | grep -q '"success":false' && printf '%s' "$RESP5" | grep -q 'is covered' && printf '%s' "$CL5" | grep -q '"data":0'; then
  report 5 "click #covered-btn (预期失败)" 1 "api-error 含 is covered; coveredClicks=0"
else
  report 5 "click #covered-btn (预期失败)" 0 "resp=$(printf '%s' "$RESP5" | head -c 200) clicks=$(printf '%s' "$CL5" | head -c 60)"
fi

# 6) hover
evaljs '(window.__hover.over=0,window.__hover.enter=0,window.__hover.leave=0,true)' >/dev/null
api hover '{"selector":"#hover-target"}' >/dev/null
sleep 0.4
case_from_verdict 6 "hover #hover-target" "$(evaljs '(function(){var el=document.getElementById("hover-target");var h=el.matches(":hover");var bg=getComputedStyle(el).backgroundColor;return (h&&bg==="rgb(43, 138, 62)")?("VERDICT ok hover="+h+" bg="+bg):("VERDICT fail hover="+h+" bg="+bg);})()')"

# 7) fill #text
evaljs '(window.__events.length=0, document.getElementById("text").value="", true)' >/dev/null
api fill '{"selector":"#text","value":"Hello 1!"}' >/dev/null
sleep 0.3
case_from_verdict 7 "fill #text" "$(evaljs '(function(){var v=document.getElementById("text").value;var bi=window.__events.some(function(x){return x.type==="beforeinput"&&x.isTrusted});var inp=window.__events.some(function(x){return x.type==="input"&&x.isTrusted});return (v==="Hello 1!"&&bi&&inp)?("VERDICT ok value="+v+" bi="+bi+" input="+inp):("VERDICT fail value="+v+" bi="+bi+" input="+inp);})()')"

# 8) fill 受控组件
evaljs '(window.__controlled.state="",window.__controlled.trustedInputs=0,document.getElementById("controlled").value="",true)' >/dev/null
api fill '{"selector":"#controlled","value":"xyz"}' >/dev/null
sleep 0.3
case_from_verdict 8 "fill #controlled (受控)" "$(evaljs '(function(){var v=document.getElementById("controlled").value;var s=window.__controlled.state;return (v==="xyz"&&s==="xyz")?("VERDICT ok value="+v+" state="+s):("VERDICT fail value="+v+" state="+s);})()')"

# 9) select
api select '{"selector":"#sel","value":"o3"}' >/dev/null
sleep 0.2
case_from_verdict 9 "select #sel o3" "$(evaljs '(function(){var v=document.getElementById("sel").value;return (v==="o3")?("VERDICT ok value="+v):("VERDICT fail value="+v);})()')"

# 10) iframe-click
evaljs 'window.__iframeClicks=0' >/dev/null
api iframe-click '{"iframeSelector":"#ifr","selector":"#ifr-btn"}' >/dev/null
sleep 0.3
case_from_verdict 10 "iframe-click #ifr #ifr-btn" "$(evaljs '(function(){var c=window.__iframeClicks;return (c>=1)?("VERDICT ok clicks="+c):("VERDICT fail clicks="+c);})()')"

# 11) upload-file（预期失败）
R11="$(api upload-file '{"selector":"#file","filePath":"/etc/hostname"}')"
if printf '%s' "$R11" | grep -q '"success":false'; then report 11 "upload-file (预期失败)" 1 "返回错误"; else report 11 "upload-file (预期失败)" 0 "resp=$(printf '%s' "$R11" | head -c 200)"; fi

# 12) drag（预期失败）
R12="$(api drag '{"sourceSelector":"#drag-src","targetSelector":"#drop-zone"}')"
if printf '%s' "$R12" | grep -q '"success":false'; then report 12 "drag (预期失败)" 1 "返回错误"; else report 12 "drag (预期失败)" 0 "resp=$(printf '%s' "$R12" | head -c 200)"; fi

# 13) navigate 慢页面（就绪）
api navigate "{\"url\":\"$(json_escape "$SLOWPAGE")\"}" >/dev/null
sleep 0.2
case_from_verdict 13 "navigate slow-page (就绪)" "$(evaljs '(function(){var f=location.href.indexOf("slow-page")>=0;var has=!!document.getElementById("slow-ready");var rs=document.readyState;return (f&&has&&rs==="complete")?("VERDICT ok hrefSlowPage="+f+" hasReady="+has+" readyState="+rs):("VERDICT fail hrefSlowPage="+f+" hasReady="+has+" readyState="+rs);})()')"

# 14) scroll 稳定
api navigate "{\"url\":\"$(json_escape "$TESTPAGE")\"}" >/dev/null
sleep 0.4
evaljs 'window.scrollTo(0,0)' >/dev/null
api scroll '{"direction":"down","amount":600}' >/dev/null
Y1="$(evaljs '"VERDICT y=" + Math.round(window.scrollY)' | grep -o 'VERDICT y=[0-9-]*' | head -1)"
sleep 0.25
Y2="$(evaljs '"VERDICT y=" + Math.round(window.scrollY)' | grep -o 'VERDICT y=[0-9-]*' | head -1)"
if [ -n "$Y1" ] && [ "$Y1" = "$Y2" ]; then report 14 "scroll 稳定" 1 "$Y1 == $Y2"; else report 14 "scroll 稳定" 0 "${Y1:-none} vs ${Y2:-none}"; fi

# 15) scroll-to-element 内层容器
api scroll-to-element '{"selector":"#sc-bottom"}' >/dev/null
sleep 0.2
case_from_verdict 15 "scroll-to-element #sc-bottom (内层)" "$(evaljs '(function(){var r=document.getElementById("sc-bottom").getBoundingClientRect();var ok=(r.bottom>0&&r.top<window.innerHeight);return ok?("VERDICT ok top="+Math.round(r.top)+" vh="+window.innerHeight):("VERDICT fail top="+Math.round(r.top)+" vh="+window.innerHeight);})()')"

# ---- 汇总 ----
TOTAL=15
log "================================"
log "verify-linux: PASS=$PASS FAIL=$FAIL / $TOTAL"
log "report: $LOG"
if [ "$FAIL" -eq 0 ]; then
  log "RESULT: ALL PASS"
  exit 0
else
  log "RESULT: FAILED ->$FAILED"
  exit 1
fi
