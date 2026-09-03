//! Tauri UI 模块:单窗口 + 四 Webview(工具栏/页面/覆盖层/面板)
//! 页面 Webview 注入 page-bridge(零注入,无残留脚本)
//! 工具栏 Webview 顶部横条(标签 + 地址栏 + 面板开关)
//! 覆盖层 Webview 透明叠加,面板 Webview 覆盖式浮层(不占页面)
pub mod annotate;
/// 新窗口拦截(Windows:target=_blank → 新标签页)
#[cfg(windows)]
pub mod new_window;

use std::sync::Mutex;

use serde::Serialize;
use tauri::window::WindowBuilder;
use tauri::webview::WebviewBuilder;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl};

/// 页面 Webview 标签
pub const PAGE_WEBVIEW: &str = "page";
/// 覆盖层 Webview 标签
pub const OVERLAY_WEBVIEW: &str = "overlay";
/// 面板 Webview 标签
pub const PANEL_WEBVIEW: &str = "panel";
/// 工具栏 Webview 标签
pub const TOOLBAR_WEBVIEW: &str = "toolbar";
/// 面板宽度(逻辑像素,覆盖式浮层)
pub const PANEL_WIDTH: f64 = 280.0;
/// 顶部工具栏高度(逻辑像素,两行:标签行 + 地址栏行)
pub const TOOLBAR_HEIGHT: f64 = 72.0;

/// 标签页状态
#[derive(Debug, Clone, Serialize)]
pub struct TabState {
    pub id: u32,
    pub url: String,
    pub title: String,
    /// 对应独立页面 Webview 的 label(真多标签,每标签一个 webview)
    #[serde(skip)]
    pub webview: Option<String>,
}

/// 预创建页面 Webview 数量(初始少量,其余动态创建;动态创建后用 SetWindowPos 置顶浮层)
pub const MAX_TABS: u32 = 3;

/// 新标签页地址(tauri 内部页面,背景跟随主题,不依赖 webview 默认背景色)
pub const NEWTAB_URL: &str = "tauri://localhost/newtab.html";

/// 待确认批注(点击元素后弹框输入,确定后入列)
#[derive(Debug, Clone)]
pub struct PendingClick {
    pub selector: String,
    pub rect: (i32, i32, i32, i32),
}

/// 待确认截图(双击全屏/框选后显示预览,保存/取消后清除)
#[derive(Debug, Clone)]
pub struct PendingShot {
    /// PNG base64(不含 data: 前缀,推送 opencode 时直接作为 file.data)
    pub image: String,
    /// 截图区域(视口 CSS 像素坐标)
    pub rect: (i32, i32, i32, i32),
}

/// 记录类型:批注
pub const RECORD_ANNOTATE: &str = "annotate";
/// 记录类型:截图
pub const RECORD_SCREENSHOT: &str = "screenshot";

/// 批注/截图记录(Rust 侧持有,推送面板)
#[derive(Debug, Clone, serde::Serialize)]
pub struct AnnotationRecord {
    pub index: u32,
    /// 记录类型:annotate(批注) / screenshot(截图)
    #[serde(rename = "type")]
    pub typ: String,
    /// 记录时所在页面地址(换地址/路由后按地址区分)
    pub url: String,
    pub selector: String,
    pub rect: (i32, i32, i32, i32),
    pub note: String,
    /// 截图记录:Png base64(仅 screenshot 类型有值)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
}

/// UI 状态(跨线程共享)
pub struct UiState {
    /// 批注模式开关
    pub annotate_mode: Mutex<bool>,
    /// 批注记录
    pub records: Mutex<Vec<AnnotationRecord>>,
    /// 待发送记录队列(面板点击"发送全部"后,插件端轮询消费)
    pub sent_records: Mutex<Vec<serde_json::Value>>,
    /// 开发者工具开关(wry 的 is_devtools_open 在 webview2 上恒 false,需自行维护)
    pub devtools_open: Mutex<bool>,
    /// HTTP 服务端口(面板 invoke 获取后 fetch /api/*)
    pub service_port: Mutex<u16>,
    /// 面板浮层显示开关(覆盖式)
    pub panel_open: Mutex<bool>,
    /// 标签页列表(伪多标签:URL 记录 + 切换导航)
    pub tabs: Mutex<Vec<TabState>>,
    /// 当前激活标签 id
    pub active_tab: Mutex<u32>,
    /// 标签 id 自增
    pub next_tab_id: Mutex<u32>,
    /// 待确认批注(点击元素后弹框确认,未提交前不入列)
    pub pending_click: Mutex<Option<PendingClick>>,
    /// 待确认截图(截图模式截取后待保存)
    pub pending_shot: Mutex<Option<PendingShot>>,
    /// 截图模式开关(截图模式下 overlay 拦截鼠标,双击截全屏/框选截区域)
    pub shot_mode: Mutex<bool>,
    /// 媒体设备模式:simulate(默认,模拟摄像头/麦克风) / real(真实设备,无设备自动回退模拟)
    pub media_mode: Mutex<String>,
    /// WebView2 用户数据目录(多用户配置隔离,来自 --user-data-dir;None=默认目录)
    pub user_data_dir: Mutex<Option<String>>,
    /// 主题模式:auto(跟随系统) / light / dark
    pub theme: Mutex<String>,
}

impl UiState {
    pub fn new() -> Self {
        Self {
            annotate_mode: Mutex::new(false),
            records: Mutex::new(Vec::new()),
            sent_records: Mutex::new(Vec::new()),
            devtools_open: Mutex::new(false),
            service_port: Mutex::new(0),
            panel_open: Mutex::new(false),
            tabs: Mutex::new(vec![TabState { id: 1, url: "about:blank".into(), title: "新标签页".into(), webview: None }]),
            active_tab: Mutex::new(1),
            next_tab_id: Mutex::new(2),
            pending_click: Mutex::new(None),
            pending_shot: Mutex::new(None),
            shot_mode: Mutex::new(false),
            media_mode: Mutex::new("simulate".into()),
            user_data_dir: Mutex::new(None),
            theme: Mutex::new("auto".into()),
        }
    }
}

/// 页面桥脚本(注入页面 Webview,提供 elementFromPoint/selector 等工具)
const PAGE_BRIDGE_JS: &str = r##"
(function(){
  function escapeCss(s){return s.replace(/([^a-zA-Z0-9_-])/g,"\\$1");}
  function buildSelector(node){
    var parts=[],cur=node;
    while(cur&&cur.nodeType===1&&cur!==document.documentElement){
      var tag=cur.tagName.toLowerCase(),part=tag;
      if(cur.id){part+="#"+escapeCss(cur.id);parts.unshift(part);break;}
      var cls=Array.prototype.slice.call(cur.classList,0,2).map(escapeCss);
      if(cls.length)part+="."+cls.join(".");
      var parent=cur.parentElement;
      if(parent){
        var sibs=Array.prototype.slice.call(parent.children);
        var same=sibs.filter(function(c){return c.tagName===cur.tagName;});
        if(same.length>1)part+=":nth-child("+(sibs.indexOf(cur)+1)+")";
      }
      parts.unshift(part);cur=parent;
    }
    return parts.join(" > ");
  }
  window.__btPage={
    query:function(x,y){
      var el=document.elementFromPoint(x,y);
      if(!el||el===document.documentElement||el===document.body)return null;
      var r=el.getBoundingClientRect();
      return {selector:buildSelector(el),rect:{x:r.x,y:r.y,w:r.width,h:r.height},tag:el.tagName.toLowerCase()};
    },
    state:function(){
      return {url:location.href,title:document.title,readyState:document.readyState,
        viewport:{w:innerWidth,h:innerHeight,dpr:devicePixelRatio}};
    }
  };
  // 控制台日志捕获:劫持 console.* 存入 __btLogs(service /api/console-logs 读取)
  window.__btLogs=[];
  ["log","info","warn","error","debug"].forEach(function(level){
    var orig=console[level]&&console[level].bind(console);
    console[level]=function(){
      var msg=Array.prototype.map.call(arguments,function(a){
        try{return (typeof a==="object"&&a!==null)?JSON.stringify(a):String(a);}catch(e){return String(a);}
      }).join(" ");
      if(window.__btLogs.length>=500)window.__btLogs.shift();
      window.__btLogs.push({level:level,msg:msg});
      if(orig)orig.apply(null,arguments);
    };
  });
})();
"##;

/// 媒体模拟脚本(注入页面 Webview,覆盖 getUserMedia 实现模拟/真实设备切换)
/// 模式:simulate(默认,fake 流不占真实硬件) / real(真实设备,无设备自动回退模拟)
/// 切换由 Rust 通过 eval 设置 window.__btMediaMode;PermissionRequested 已在 Rust 侧统一放行
const MEDIA_FAKE_JS: &str = r##"
(function(){
  var media=navigator.mediaDevices;
  if(!media)return;
  window.__btMediaMode="simulate";
  var _orig=media.getUserMedia&&media.getUserMedia.bind(media);
  function makeFake(constraints){
    return new Promise(function(resolve){
      try{
        // 视频轨取自共享模拟摄像头总线(所有 fake 流共用,外部可注入图片/视频)
        var video=ensureCam().canvas.captureStream(15).getVideoTracks()[0];
        // 音频轨取自共享模拟总线(所有 fake 流共用,外部可注入声音)
        var audio=ensureMic().dst.stream.getAudioTracks()[0];
        var tracks=[];
        if(!constraints.video||constraints.video!==false)tracks.push(video);
        if(!constraints.audio||constraints.audio!==false)tracks.push(audio);
        resolve(new MediaStream(tracks));
      }catch(e){resolve(new MediaStream());}
    });
  }
  // ---- 共享模拟摄像头总线(fake 摄像头) ----
  // 所有 fake 流的视频轨取自同一个 canvas 的 captureStream;
  // 默认多场景轮换(mode=auto),外部可注入静态图片(mode=image)/视频流(mode=video)或冻结(stop)
  var cam=null;
  function ensureCam(){
    if(cam)return cam;
    var w=640,h=480,c=document.createElement("canvas");c.width=w;c.height=h;
    var ctx=c.getContext("2d");
    var camObj={canvas:c,ctx:ctx,mode:"auto",videoEl:null};
    cam=camObj;
    // 场景函数签名:function(ctx,w,h,t,now)  t=场景内已运行秒数  now=当前时间戳
    // 场景1:弹跳球乐园(渐变动画 + 彩色弹跳球 + 顶部时钟)
    function sceneBalls(ctx,w,h,t,now){
      var hue=(t*40)%360;
      var g=ctx.createLinearGradient(0,0,w,h);
      g.addColorStop(0,"hsl("+hue+",70%,55%)");
      g.addColorStop(1,"hsl("+((hue+120)%360)+",70%,45%)");
      ctx.fillStyle=g;ctx.fillRect(0,0,w,h);
      var balls=[
        {x:0.30,y:0.30,vx:80,vy:60,r:28,c:"#ffd93d"},
        {x:0.70,y:0.60,vx:-70,vy:50,r:20,c:"#ff6b6b"},
        {x:0.50,y:0.80,vx:60,vy:-80,r:16,c:"#4ecdc4"},
        {x:0.20,y:0.50,vx:-50,vy:70,r:22,c:"#a29bfe"},
        {x:0.80,y:0.20,vx:55,vy:-60,r:18,c:"#ffe28a"}
      ];
      ctx.textAlign="left";
      for(var i=0;i<balls.length;i++){
        var b=balls[i];
        var px=(b.x*w+b.vx*t)%(2*w);if(px<0)px+=2*w;var xx=px<w?px:2*w-px;
        var py=(b.y*h+b.vy*t)%(2*h);if(py<0)py+=2*h;var yy=py<h?py:2*h-py;
        ctx.beginPath();ctx.arc(xx,yy,b.r,0,Math.PI*2);
        ctx.fillStyle=b.c;ctx.fill();
        ctx.strokeStyle="rgba(255,255,255,.55)";ctx.lineWidth=3;ctx.stroke();
      }
      ctx.fillStyle="rgba(0,0,0,.35)";ctx.fillRect(0,0,w,52);
      ctx.fillStyle="#fff";ctx.font="bold 22px sans-serif";
      ctx.fillText("FAKE CAMERA",16,35);
      ctx.font="bold 24px monospace";ctx.textAlign="right";
      ctx.fillText(new Date(now).toLocaleTimeString(),w-16,36);
    }
    // 场景2:电视测试卡(SMPTE 七色彩条 + 网格 + 时间码)
    function sceneTestcard(ctx,w,h,t,now){
      ctx.fillStyle="#1a1a2e";ctx.fillRect(0,0,w,h);
      var cols=["#c0c0c0","#c0c000","#00c0c0","#00c000","#c000c0","#c00000","#0000c0"];
      var bw=w/cols.length,bh=h*0.55;
      for(var i=0;i<cols.length;i++){ctx.fillStyle=cols[i];ctx.fillRect(i*bw,0,bw,bh);}
      var grad=ctx.createLinearGradient(0,bh,0,bh+h*0.2);
      grad.addColorStop(0,"#000");grad.addColorStop(1,"#fff");
      ctx.fillStyle=grad;ctx.fillRect(0,bh,w,h*0.2);
      ctx.strokeStyle="#333";ctx.lineWidth=1;
      for(var x=0;x<=w;x+=w/8){ctx.beginPath();ctx.moveTo(x,bh);ctx.lineTo(x,bh+h*0.2);ctx.stroke();}
      ctx.fillStyle="#111";ctx.fillRect(0,h-56,w,56);
      ctx.strokeStyle="#222";
      for(var y=0;y<3;y++){ctx.beginPath();ctx.moveTo(0,h-56+y*14);ctx.lineTo(w,h-56+y*14);ctx.stroke();}
      ctx.fillStyle="#fff";ctx.font="bold 26px monospace";ctx.textAlign="center";
      var d=new Date(now);
      var ts=("0"+d.getHours()).slice(-2)+":"+("0"+d.getMinutes()).slice(-2)+":"+("0"+d.getSeconds()).slice(-2);
      ctx.fillText("TIME "+ts,w/2,h-22);
      ctx.font="bold 15px sans-serif";
      ctx.fillText("FAKE CAMERA TEST CARD",w/2,h-44);
    }
    // 场景3:表情动效(大号表情轮换 + 彩带飘落)
    function sceneEmoji(ctx,w,h,t,now){
      var g=ctx.createLinearGradient(0,0,w,h);
      g.addColorStop(0,"#2b1055");g.addColorStop(1,"#7597de");
      ctx.fillStyle=g;ctx.fillRect(0,0,w,h);
      var emo=["😀","😎","🤖","🎉","🐱","🌈"];
      var e=emo[Math.floor(now/800)%emo.length];
      ctx.textAlign="center";ctx.textBaseline="middle";
      ctx.font="150px sans-serif";
      ctx.fillText(e,w/2,h/2-20);
      ctx.textBaseline="alphabetic";
      for(var i=0;i<24;i++){
        var p=((i*137+now*0.05)%1);
        var px=((i*83+Math.sin(now/900+i)*40)%w);if(px<0)px+=w;
        var py=p*(h-60);
        ctx.globalAlpha=0.7;
        ctx.fillStyle=["#ff6b6b","#ffd93d","#4ecdc4","#a29bfe"][i%4];
        ctx.fillRect(px,py,10,10);
      }
      ctx.globalAlpha=1;
      ctx.fillStyle="rgba(0,0,0,.35)";ctx.fillRect(0,h-48,w,48);
      ctx.fillStyle="#fff";ctx.font="bold 20px sans-serif";
      ctx.fillText("FAKE CAMERA",w/2,h-16);
    }
    // 场景4:流星夜空(径向渐变 + 星星 + 周期性流星划过)
    function sceneStars(ctx,w,h,t,now){
      var g=ctx.createRadialGradient(w/2,h/2,40,w/2,h/2,Math.max(w,h)*0.65);
      g.addColorStop(0,"#0f0c29");g.addColorStop(0.5,"#302b63");g.addColorStop(1,"#24243e");
      ctx.fillStyle=g;ctx.fillRect(0,0,w,h);
      // 静止星星(微弱,衬托流星)
      for(var i=0;i<60;i++){
        var sx=(i*97.3+13)%w,sy=(i*61.7+29)%h;
        var tw=0.5+0.5*Math.sin(now/400+i);
        ctx.globalAlpha=tw*0.6;
        ctx.fillStyle="#fff";
        ctx.fillRect(sx,sy,1+((i*7)%2),1+((i*7)%2));
      }
      // 流星:多个不同起点/角度/速度,按 gap 周期持续划过,头部亮+拖尾渐变
      var meteors=[
        {st:0,   dur:1200, gap:2600, x:0.10, y:0.05, dx:0.55, dy:0.18, len:120, c:"#ffffff"},
        {st:900, dur:1000, gap:2200, x:0.90, y:0.10, dx:-0.50, dy:0.22, len:90, c:"#cfe8ff"},
        {st:300, dur:1400, gap:2800, x:0.20, y:0.02, dx:0.45, dy:0.15, len:150, c:"#ffe9c4"},
        {st:1500,dur:900,  gap:2500, x:0.60, y:0.06, dx:-0.60, dy:0.20, len:100, c:"#e6d9ff"}
      ];
      for(var m=0;m<meteors.length;m++){
        var mt=meteors[m];
        var age=(now+mt.st)%mt.gap;
        if(age>mt.dur)continue;              // 未到或已划过
        var p=age/mt.dur;                     // 0..1 划过进度
        var fade=Math.min(p/0.12,(1-p)/0.12,1);
        if(fade<=0)continue;
        var hx=mt.x*w+mt.dx*w*p, hy=mt.y*h+mt.dy*h*p;
        var tx=hx-mt.dx*mt.len, ty=hy-mt.dy*mt.len;
        // 尾迹渐变(头部亮 → 尾端透明)
        var gr=ctx.createLinearGradient(hx,hy,tx,ty);
        gr.addColorStop(0,mt.c);
        gr.addColorStop(1,"rgba(255,255,255,0)");
        ctx.globalAlpha=fade;
        ctx.strokeStyle=gr;
        ctx.lineWidth=2;
        ctx.beginPath();ctx.moveTo(hx,hy);ctx.lineTo(tx,ty);ctx.stroke();
        // 头部亮点
        ctx.fillStyle="#fff";
        ctx.beginPath();ctx.arc(hx,hy,2.5,0,Math.PI*2);ctx.fill();
      }
      ctx.globalAlpha=1;
      ctx.fillStyle="rgba(0,0,0,.35)";ctx.fillRect(0,h-48,w,48);
      ctx.fillStyle="#fff";ctx.font="bold 20px sans-serif";ctx.textAlign="center";
      ctx.fillText("FAKE CAMERA · METEOR SHOWER",w/2,h-16);
      ctx.textAlign="left";
    }
    // 场景调度:每 15 秒轮换;非 auto 模式画注入内容或保留静态帧
    var scenes=[sceneBalls,sceneTestcard,sceneEmoji,sceneStars];
    var si=0,start=Date.now();
    function draw(){
      requestAnimationFrame(draw);
      var now=Date.now();
      if(camObj.mode==="auto"){
        if(now-start>15000){start=now;si=(si+1)%scenes.length;}
        scenes[si](ctx,w,h,(now-start)/1000,now);
      }else if(camObj.mode==="video"&&camObj.videoEl){
        // 视频注入:每帧把视频画面画到 canvas
        ctx.drawImage(camObj.videoEl,0,0,w,h);
      }else if(camObj.mode==="image"&&camObj.imageEl){
        // 图片注入:每帧重绘注入图(持续提交画布,否则 captureStream 会停流出帧)
        ctx.drawImage(camObj.imageEl,0,0,w,h);
      }else if(camObj.mode==="frozen"&&camObj.frozen){
        // 冻结帧:每帧重放快照(保持画布持续提交,流不中断)
        ctx.putImageData(camObj.frozen,0,0);
      }
    }
    draw();
    return camObj;
  }
  // 外部注入 API(由 Rust /api/media/video 调用;init 脚本注入后本对象恒存在)
  window.__btFakeCam={
    ensure:function(){return ensureCam();},
    // 注入静态图片(base64 或 dataURL),停止场景轮换
    setImage:function(dataB64){
      var c=ensureCam();
      c.mode="image";
      c.frozen=null;
      if(c.videoEl){c.videoEl.pause();c.videoEl=null;}
      var img=new Image();
      img.onload=function(){c.imageEl=img;};
      img.src=dataB64.indexOf("data:")===0?dataB64:"data:image/png;base64,"+dataB64;
    },
    // 注入视频流(url 或 dataURL),停止场景轮换;loop=true(默认)循环播放,false 播完冻结末帧
    setVideo:function(url,loop){
      var c=ensureCam();
      c.mode="video";
      c.imageEl=null;c.frozen=null;
      if(c.videoEl)c.videoEl.pause();
      var v=document.createElement("video");
      v.src=url;v.loop=loop!==false;v.muted=true;v.autoplay=true;v.playsInline=true;v.crossOrigin="anonymous";
      v.play().catch(function(){});
      c.videoEl=v;
    },
    // 恢复默认场景轮换
    setAuto:function(){ensureCam().mode="auto";},
    // 停止注入/冻结当前帧(快照后持续重放,保持流活跃)
    stop:function(){
      var c=ensureCam();
      c.frozen=c.ctx.getImageData(0,0,c.canvas.width,c.canvas.height);
      c.mode="frozen";
    }
  };
  // ---- 共享模拟音频总线(fake 麦克风) ----
  // 所有 fake 流的音频轨取自同一个 destination,外部可向总线注入声音;
  // 默认随机循环播放不同类型可听声音,便于判断音频通路是否正常
  var mic=null;
  function ensureMic(){
    if(mic)return mic;
    var ctx=new (window.AudioContext||window.webkitAudioContext)();
    var dst=ctx.createMediaStreamDestination();
    mic={ctx:ctx,dst:dst,timer:null};
    // 尝试恢复自动播放限制(无用户手势时可能 suspended,有手势后自动恢复)
    if(ctx.state==="suspended"){ctx.resume().catch(function(){});}
    startAmbient();
    return mic;
  }
  function stopAmbient(){
    if(mic&&mic.timer){clearTimeout(mic.timer);mic.timer=null;}
  }
  // 默认随机音效循环:噪声脉冲/双音提示/DTMF/旋律,间隔随机
  function startAmbient(){
    var m=ensureMic();
    if(m.timer)return;
    var seq=[
      function(){noiseBurst(m,0.6,0.18);},
      function(){beep(m,880,140);setTimeout(function(){if(mic===m)beep(m,1320,140);},200);},
      function(){dtmf(m,"13579");},
      function(){melody(m);},
      function(){noiseBurst(m,1.0,0.12);},
      function(){beep(m,523,260);}
    ];
    var i=0;
    function next(){
      if(!mic||mic!==m)return;
      seq[i%seq.length]();i++;
      m.timer=setTimeout(next,1500+Math.random()*1800);
    }
    next();
  }
  // 生成带淡入淡出的噪声 buffer
  function noiseBuf(ctx,dur){
    var rate=ctx.sampleRate;
    var buf=ctx.createBuffer(1,Math.floor(rate*dur),rate);
    var d=buf.getChannelData(0);
    for(var i=0;i<d.length;i++){
      var edge=Math.min(i/160,(d.length-i)/160);
      d[i]=(Math.random()*2-1)*Math.max(0,Math.min(1,edge));
    }
    return buf;
  }
  // 单音(正弦,带音量包络)
  function beep(m,freq,dur){
    var t=m.ctx.currentTime;
    var osc=m.ctx.createOscillator();
    var g=m.ctx.createGain();
    osc.type="sine";osc.frequency.value=freq;
    g.gain.setValueAtTime(0.001,t);
    g.gain.exponentialRampToValueAtTime(0.45,t+0.012);
    g.gain.exponentialRampToValueAtTime(0.001,t+dur/1000);
    osc.connect(g);g.connect(m.dst);
    osc.start(t);osc.stop(t+dur/1000+0.03);
  }
  // 音调序列:notes=[[f1,f2,durMs],...],f2 可省(单音);支持旋律与双音(DTMF)
  function playSeq(m,notes){
    var t=m.ctx.currentTime;
    for(var i=0;i<notes.length;i++){
      var n=notes[i];
      var fs=[n[0],n[1]];
      for(var j=0;j<fs.length;j++){
        var f=fs[j];if(!f)continue;
        var o=m.ctx.createOscillator();
        var g=m.ctx.createGain();
        o.frequency.value=f;
        g.gain.setValueAtTime(0.001,t);
        g.gain.exponentialRampToValueAtTime(0.4,t+0.01);
        g.gain.exponentialRampToValueAtTime(0.001,t+n[2]/1000);
        o.connect(g);g.connect(m.dst);
        o.start(t);o.stop(t+n[2]/1000+0.03);
      }
      t+=n[2]/1000+0.03;
    }
  }
  // 噪声脉冲
  function noiseBurst(m,dur,amp){
    var src=m.ctx.createBufferSource();
    src.buffer=noiseBuf(m.ctx,dur);
    var g=m.ctx.createGain();g.gain.value=amp||0.2;
    src.connect(g);g.connect(m.dst);
    src.start();
  }
  // DTMF 拨号音(双音)
  function dtmf(m,digits){
    var f={
      "1":[697,1209],"2":[697,1336],"3":[697,1477],
      "4":[770,1209],"5":[770,1336],"6":[770,1477],
      "7":[852,1209],"8":[852,1336],"9":[852,1477],
      "0":[941,1336],"*":[941,1209],"#":[941,1477]
    };
    var notes=[];
    for(var i=0;i<digits.length;i++){
      var p=f[digits[i]];if(!p)continue;
      notes.push([p[0],p[1],110]);
    }
    if(notes.length)playSeq(m,notes);
  }
  // 简单旋律(小星星前两句)
  function melody(m){
    playSeq(m,[
      [523,0,300],[523,0,300],[784,0,300],[784,0,300],
      [880,0,300],[880,0,300],[784,0,600]
    ]);
  }
  // 播放解码后的 AudioBuffer(先停环境音,保证注入声音清晰);loop=true 循环播放
  function playBuffer(m,buf,loop){
    stopAmbient();
    var src=m.ctx.createBufferSource();
    src.buffer=buf;src.loop=!!loop;src.connect(m.dst);src.start();
  }
  // 外部注入 API(由 Rust /api/media/audio 调用;init 脚本注入后本对象恒存在)
  window.__btFakeMic={
    ensure:function(){return ensureMic();},
    // 注入 base64 音频(裸 base64 或 dataURL)→ decodeAudioData → 播放进 fake 麦克风
    // loop=true 循环播放,false 单次
    inject:function(dataB64,loop){
      try{
        var m=ensureMic();
        var raw=dataB64.indexOf(",")>=0?dataB64.split(",")[1]:dataB64;
        var bin=atob(raw);
        var bytes=new Uint8Array(bin.length);
        for(var i=0;i<bin.length;i++)bytes[i]=bin.charCodeAt(i);
        m.ctx.decodeAudioData(bytes.buffer,function(buf){
          if(mic!==m)return;
          playBuffer(m,buf,loop);
        },function(){});
      }catch(e){}
    },
    tone:function(freq,dur){stopAmbient();beep(ensureMic(),freq,dur);},
    seq:function(notes){stopAmbient();playSeq(ensureMic(),notes);},
    dtmf:function(digits){stopAmbient();dtmf(ensureMic(),digits);},
    noise:function(dur){stopAmbient();noiseBurst(ensureMic(),dur,0.25);},
    ambient:function(){startAmbient();},
    stop:function(){stopAmbient();}
  };
  media.getUserMedia=function(constraints){
    if(window.__btMediaMode!=="real"){
      return makeFake(constraints||{});
    }
    if(_orig){
      return _orig(constraints).catch(function(){return makeFake(constraints||{});});
    }
    return makeFake(constraints||{});
  };
})();
"##;

/// 按需给 WebviewBuilder 设置用户数据目录(多用户配置隔离;None 用默认目录)
/// 同目录的多个 webview 会共享同一 WebView2 environment,页面/工具栏等统一走同一用户配置
fn with_data_dir<R: Runtime>(builder: WebviewBuilder<R>, dir: &Option<String>) -> WebviewBuilder<R> {
    match dir {
        Some(d) => builder.data_directory(std::path::PathBuf::from(d.as_str())),
        None => builder,
    }
}

/// 创建主窗口 + 四 Webview(工具栏/页面/覆盖层/面板)
/// 布局以窗口实际物理尺寸为准(避免 DPI 感知时序导致窗口与子 webview 缩放不一致)
pub fn create_ui(app: &AppHandle) -> tauri::Result<()> {
    // 用户数据目录(多用户配置):所有 webview 共用同一目录 → 共享同一用户配置
    let data_dir = app.state::<UiState>().user_data_dir.lock().unwrap().clone();
    // 主窗口(逻辑 1100x700:150% DPI 下物理 1650x1050,适配常见 1920x1080 屏幕)
    // 最小尺寸 500x400(逻辑像素),防止窗口拖到过小导致布局崩溃
    // 背景深色避免启动时白屏闪烁
    let window = WindowBuilder::new(app, "main")
        .title("bt-shell")
        .inner_size(1100.0, 700.0)
        .min_inner_size(500.0, 400.0)
        .background_color(tauri::window::Color(20, 20, 20, 255))
        // 无系统标题栏(窗口控制按钮移到工具栏标签行右侧,布局更紧凑)
        .decorations(false)
        .build()?;

    // 1. 工具栏 Webview(顶部横条:标签 + 地址栏 + 面板开关)
    // 1. 工具栏 Webview(顶部横条:标签 + 地址栏 + 面板开关)
    // 创建后先隐藏,布局定位完成后再显示,避免初始 100x100 在左上角闪现
    let toolbar = window.add_child(
        with_data_dir(WebviewBuilder::new(TOOLBAR_WEBVIEW, WebviewUrl::App("toolbar.html".into())), &data_dir),
        tauri::PhysicalPosition::new(0, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;
    let _ = toolbar.hide();

    // 2. 页面 Webview(工具栏下方,渲染目标网页 + 注入页面桥)
    // 真多标签:预创建 MAX_TABS 个 page webview(全部先隐藏,布局定位后仅显示第 1 个),
    // 保证创建顺序 toolbar → page-N → overlay → panel,使 overlay/panel 位于最上
    for i in 1..=MAX_TABS {
        let label = format!("{PAGE_WEBVIEW}-{i}");
        let w = window.add_child(
            with_data_dir(
                WebviewBuilder::new(
                    &label,
                    // 初始加载自定义新标签页(背景跟随主题,不依赖 webview 默认背景)
                    WebviewUrl::App("newtab.html".into()),
                )
                .initialization_script(format!("{PAGE_BRIDGE_JS}{MEDIA_FAKE_JS}")),
                &data_dir,
            ),
            tauri::PhysicalPosition::new(0, 0),
            tauri::PhysicalSize::new(100, 100),
        )?;
        // 全部先隐藏,避免布局定位前在左上角闪现 100x100 黑框
        let _ = w.hide();
        // 注册新窗口拦截(target=_blank → 新标签页)
        #[cfg(windows)]
        new_window::setup(app, &w);
        // 注册响应捕获(expect/assert-response 查询历史)
        #[cfg(windows)]
        crate::control::responses::setup(app, &w);
        // 注册媒体权限放行(摄像头/麦克风统一 Allow)
        #[cfg(windows)]
        crate::control::media::setup(app, &w);
    }
    // 初始标签绑定 webview label
    {
        let state = app.state::<UiState>();
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(t) = tabs.first_mut() {
            t.webview = Some(format!("{PAGE_WEBVIEW}-1"));
        }
    }

    // 3. 覆盖层 Webview(透明,叠加在页面区)
    let overlay = window.add_child(
        with_data_dir(
            WebviewBuilder::new(OVERLAY_WEBVIEW, WebviewUrl::App("overlay.html".into()))
                .transparent(true)
                .disable_drag_drop_handler(),
            &data_dir,
        ),
        tauri::PhysicalPosition::new(0, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;
    // 初始隐藏覆盖层:默认非批注模式,隐藏时不拦截鼠标(页面可正常交互),批注模式开启时显示
    overlay.hide()?;

    // 4. 面板 Webview(覆盖式浮层,默认隐藏,由工具栏按钮切换)
    window.add_child(
        with_data_dir(WebviewBuilder::new(PANEL_WEBVIEW, WebviewUrl::App("index.html".into())), &data_dir),
        tauri::PhysicalPosition::new(100, 0),
        tauri::PhysicalSize::new(100, 100),
    )?;

    // 初始布局按窗口实际尺寸重排
    let size = window.inner_size()?;
    let scale = window.scale_factor().unwrap_or(1.0);
    apply_layout(app, size, scale)?;
    // 布局定位完成后显示工具栏与初始标签 page-1(其余预创建 webview 保持隐藏)
    if let Some(w) = app.get_webview(TOOLBAR_WEBVIEW) {
        let _ = w.show();
    }
    if let Some(w) = app.get_webview("page-1") {
        let _ = w.show();
    }

    // 监听窗口 resize/DPI 变化 → 重排 webview
    let handle = app.clone();
    window.on_window_event(move |event| match event {
        tauri::WindowEvent::Resized(size) | tauri::WindowEvent::ScaleFactorChanged { new_inner_size: size, .. } => {
            if let Some(win) = handle.get_window("main") {
                let scale = win.scale_factor().unwrap_or(1.0);
                let _ = apply_layout(&handle, *size, scale);
            }
        }
        _ => {}
    });

    Ok(())
}

/// 重排 webview 的边界
/// 工具栏占顶部固定高度;所有页面 Webview(每标签一个)占工具栏下方全宽;
/// 覆盖层叠加在页面区;面板为右侧覆盖式浮层(关闭时尺寸归零)
/// size 为窗口物理内尺寸,scale 为窗口缩放因子
pub fn apply_layout(app: &AppHandle, size: tauri::PhysicalSize<u32>, scale: f64) -> tauri::Result<()> {
    let toolbar_h = (TOOLBAR_HEIGHT * scale) as i32;
    let panel_w = (PANEL_WIDTH * scale) as i32;
    let page_w = size.width as i32;
    let page_h = size.height as i32 - toolbar_h;

    // 1. 工具栏 Webview(顶部横条)
    if let Some(w) = app.get_webview(TOOLBAR_WEBVIEW) {
        w.set_position(tauri::PhysicalPosition::new(0, 0))?;
        w.set_size(tauri::PhysicalSize::new(page_w.max(0) as u32, toolbar_h.max(0) as u32))?;
    }

    // 2. 所有页面 Webview(每标签一个,定位到工具栏下方全宽)
    {
        let state = app.state::<UiState>();
        let labels: Vec<String> = state
            .tabs
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| t.webview.clone())
            .collect();
        for label in labels {
            if let Some(w) = app.get_webview(&label) {
                w.set_position(tauri::PhysicalPosition::new(0, toolbar_h.max(0)))?;
                w.set_size(tauri::PhysicalSize::new(page_w.max(0) as u32, page_h.max(0) as u32))?;
            }
        }
    }

    // 3. 覆盖层 Webview(透明,叠加在页面区)
    if let Some(w) = app.get_webview(OVERLAY_WEBVIEW) {
        w.set_position(tauri::PhysicalPosition::new(0, toolbar_h.max(0)))?;
        w.set_size(tauri::PhysicalSize::new(page_w.max(0) as u32, page_h.max(0) as u32))?;
    }

    // 4. 面板 Webview(右侧覆盖浮层,panel_open 关闭时尺寸归零隐藏)
    if let Some(w) = app.get_webview(PANEL_WEBVIEW) {
        let open = app.state::<UiState>().panel_open.lock().unwrap().clone();
        let (px, pw) = if open {
            (page_w - panel_w, panel_w)
        } else {
            (page_w, 0)
        };
        w.set_position(tauri::PhysicalPosition::new(px.max(0), toolbar_h.max(0)))?;
        w.set_size(tauri::PhysicalSize::new(pw.max(0) as u32, page_h.max(0) as u32))?;
    }
    // 覆盖层/面板重新置顶:动态创建的页面 webview 会排在它们之上,导致切换标签后面板被盖
    #[cfg(windows)]
    bring_webviews_to_top(app);
    Ok(())
}

/// 覆盖层/面板置顶(Windows):每个 webview 有独立 host HWND(controller 的父窗口),
/// 动态创建的页面 webview 会排在 overlay/panel 之上,用 SetWindowPos 把浮层提到最上
#[cfg(windows)]
pub fn bring_webviews_to_top(app: &AppHandle) {
    use tauri::Manager;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOMOVE, SWP_NOSIZE};
    // 先 overlay 后 panel,panel 最终在最上
    for label in [OVERLAY_WEBVIEW, PANEL_WEBVIEW] {
        if let Some(w) = app.get_webview(label) {
            let _ = w.with_webview(move |platform_webview| {
                unsafe {
                    let controller = platform_webview.controller();
                    let mut parent: HWND = HWND::default();
                    if controller.ParentWindow(&mut parent).is_ok() {
                        let _ = SetWindowPos(parent, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                    }
                }
            });
        }
    }
}

/// 获取激活标签的页面 Webview(真多标签:按 active_tab 的 webview label)
pub fn active_page_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    let state = app.state::<UiState>();
    let active = *state.active_tab.lock().unwrap();
    let label = state
        .tabs
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.id == active)
        .and_then(|t| t.webview.clone())?;
    app.get_webview(&label)
}

/// 动态创建标签页面 Webview(真多标签,每标签一个,初始隐藏)
pub fn create_tab_webview(app: &AppHandle, id: u32) -> Result<tauri::webview::Webview, String> {
    // add_child 定义在 tauri::Window(需 get_window,而非 get_webview_window)
    let window = app.get_window("main").ok_or("main window not found")?;
    // 用户数据目录与预创建 webview 保持一致(共享同一 WebView2 environment)
    let data_dir = app.state::<UiState>().user_data_dir.lock().unwrap().clone();
    let label = format!("{PAGE_WEBVIEW}-{id}");
    let w = window
        .add_child(
            with_data_dir(
                WebviewBuilder::new(&label, WebviewUrl::App("newtab.html".into()))
                    .initialization_script(format!("{PAGE_BRIDGE_JS}{MEDIA_FAKE_JS}")),
                &data_dir,
            ),
            tauri::PhysicalPosition::new(0, 0),
            tauri::PhysicalSize::new(100, 100),
        )
        .map_err(|e| format!("create page webview failed: {e}"))?;
    // 初始隐藏,切换到该标签时才显示
    let _ = w.hide();
    // 注册新窗口拦截(target=_blank → 新标签页)
    #[cfg(windows)]
    new_window::setup(app, &w);
    // 注册响应捕获(expect/assert-response 查询历史)
    #[cfg(windows)]
    crate::control::responses::setup(app, &w);
    // 注册媒体权限放行(摄像头/麦克风统一 Allow)
    #[cfg(windows)]
    crate::control::media::setup(app, &w);
    // 应用布局(定位到页面区)
    if let Some(win) = app.get_window("main") {
        if let Ok(size) = win.inner_size() {
            let scale = win.scale_factor().unwrap_or(1.0);
            let _ = apply_layout(app, size, scale);
        }
    }
    Ok(w)
}

/// 获取覆盖层 Webview
pub fn overlay_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(OVERLAY_WEBVIEW)
}

/// 获取面板 Webview
pub fn panel_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(PANEL_WEBVIEW)
}

/// 获取工具栏 Webview
pub fn toolbar_webview(app: &AppHandle) -> Option<tauri::webview::Webview> {
    app.get_webview(TOOLBAR_WEBVIEW)
}

/// 同步激活标签的 URL/标题(页面导航后调用,保持工具栏显示一致)
pub fn sync_active_tab(app: &AppHandle, url: &str, title: &str) {
    let state = app.state::<UiState>();
    let active = *state.active_tab.lock().unwrap();
    let mut tabs = state.tabs.lock().unwrap();
    if let Some(t) = tabs.iter_mut().find(|t| t.id == active) {
        if !url.is_empty() {
            t.url = url.to_string();
        }
        if !title.is_empty() {
            t.title = title.to_string();
        }
    }
}

/// 组装并广播标签状态(tabs/active/panel_open),前端监听 "tabs-changed" 更新
/// 所有会改变标签/面板状态的路径都必须调用,保持工具栏/面板 UI 同步
pub fn emit_tabs_changed(app: &AppHandle) {
    use tauri::Emitter;
    let state = app.state::<UiState>();
    let tabs = state.tabs.lock().unwrap().clone();
    let active = *state.active_tab.lock().unwrap();
    let panel_open = *state.panel_open.lock().unwrap();
    let _ = app.emit(
        "tabs-changed",
        serde_json::json!({ "tabs": tabs, "active": active, "panel_open": panel_open }),
    );
}

/// 打开面板:展开尺寸 + 遮罩盖页面区 + 广播状态
pub fn open_panel(app: &AppHandle) {
    {
        let state = app.state::<UiState>();
        *state.panel_open.lock().unwrap() = true;
    }
    if let Some(win) = app.get_window("main") {
        if let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) {
            let _ = apply_layout(app, size, scale);
        }
    }
    let _ = eval_overlay(app, "window.__btOverlay.showMask()");
    emit_tabs_changed(app);
}

/// 仅收起面板(不退出模式/不动覆盖层):开启批注/截图模式时内部自动关面板用
pub fn collapse_panel(app: &AppHandle) {
    {
        let state = app.state::<UiState>();
        *state.panel_open.lock().unwrap() = false;
    }
    let _ = eval_overlay(app, "window.__btOverlay.hideMask()");
    if let Some(win) = app.get_window("main") {
        if let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) {
            let _ = apply_layout(app, size, scale);
        }
    }
    emit_tabs_changed(app);
}

/// 关闭面板(完整):收起面板 + 取消批注/截图模式 + 关闭对应弹窗 + 清理待确认
/// 若存在未确认的截图预览/批注弹框(如"立即截图"后),保留覆盖层使其可见
pub fn close_panel(app: &AppHandle) {
    let state = app.state::<UiState>();
    *state.panel_open.lock().unwrap() = false;
    let _ = eval_overlay(app, "window.__btOverlay.hideMask()");
    // 取消批注/截图模式(退出时覆盖层隐藏,批注弹框/截图预览随模式关闭)
    let annotate = *state.annotate_mode.lock().unwrap();
    if annotate {
        let _ = annotate::Annotator::toggle(app, &state);
    }
    let shot = *state.shot_mode.lock().unwrap();
    if shot {
        let _ = annotate::Annotator::toggle_shot(app, &state);
    }
    // 存在未确认预览(立即截图/批注弹框)时保留覆盖层,否则隐藏
    let has_pending = state.pending_shot.lock().unwrap().is_some()
        || state.pending_click.lock().unwrap().is_some();
    // 清理待确认
    *state.pending_click.lock().unwrap() = None;
    *state.pending_shot.lock().unwrap() = None;
    if !has_pending {
        if let Some(overlay) = overlay_webview(app) {
            let _ = overlay.hide();
        }
    }
    if let Some(win) = app.get_window("main") {
        if let (Ok(size), Ok(scale)) = (win.inner_size(), win.scale_factor()) {
            let _ = apply_layout(app, size, scale);
        }
    }
    emit_tabs_changed(app);
}

/// 发送所有记录:快照入 sent_records 队列(插件端轮询 consume)并清空已发送记录,返回记录数量
/// 面板"发送"、批注/截图"发送"按钮共用
pub fn send_all_records(app: &AppHandle) -> usize {
    let state = app.state::<UiState>();
    let records = state.records.lock().unwrap();
    let count = records.len();
    let items: Vec<serde_json::Value> = records
        .iter()
        .map(|r| {
            serde_json::json!({
                "index": r.index,
                "type": r.typ,
                "url": r.url,
                "selector": r.selector,
                "rect": [r.rect.0, r.rect.1, r.rect.2, r.rect.3],
                "note": r.note,
                "image": r.image,
            })
        })
        .collect();
    drop(records);
    // 清空已发送的记录(发送后不再保留)
    state.records.lock().unwrap().clear();
    *state.sent_records.lock().unwrap() = items;
    // 通知面板刷新(空列表)
    use tauri::Emitter;
    if let Some(panel) = panel_webview(app) {
        let _ = panel.emit("records-changed", Vec::<AnnotationRecord>::new());
    }
    // 清空覆盖层批注/截图标记
    let _ = eval_overlay(app, "window.__btOverlay._marks = []; window.__btOverlay.redraw([], [])");
    count
}

/// 获取当前激活标签的页面地址(记录归属地址)
pub fn active_tab_url(app: &AppHandle) -> String {
    let state = app.state::<UiState>();
    let active = *state.active_tab.lock().unwrap();
    state
        .tabs
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.id == active)
        .map(|t| t.url.clone())
        .unwrap_or_default()
}

/// 标签切换/新建前清理:退出批注/截图模式并关闭面板
/// (覆盖层停留在旧页面,切换/新建标签后需清理,避免残留遮挡)
pub fn exit_modes_and_close_panel(app: &AppHandle) {
    let state = app.state::<UiState>();
    let annotate = *state.annotate_mode.lock().unwrap();
    if annotate {
        let _ = annotate::Annotator::toggle(app, &state);
    }
    let shot = *state.shot_mode.lock().unwrap();
    if shot {
        let _ = annotate::Annotator::toggle_shot(app, &state);
    }
    if *state.panel_open.lock().unwrap() {
        close_panel(app);
    }
}

/// 打开新标签页:创建独立页面 Webview 并导航(工具栏"+"与页面 target=_blank/window.open 事件共用)
pub fn open_new_tab(app: &AppHandle, url: &str) -> Result<(), String> {
    // 新建标签前清理:退出批注/截图模式并关闭面板
    exit_modes_and_close_panel(app);
    let url = normalize_url(url);
    let state = app.state::<UiState>();
    let id = {
        let mut n = state.next_tab_id.lock().unwrap();
        let id = *n;
        *n += 1;
        id
    };
    // 隐藏当前激活 webview
    if let Some(cur) = active_page_webview(app) {
        let _ = cur.hide();
    }
    // 优先复用预创建 webview,超出 MAX_TABS 则动态创建
    let label = format!("{PAGE_WEBVIEW}-{id}");
    let wv = match app.get_webview(&label) {
        Some(w) => w,
        None => create_tab_webview(app, id)?,
    };
    wv.show().map_err(|e| format!("show webview failed: {e}"))?;
    // 记录标签并导航
    {
        let mut tabs = state.tabs.lock().unwrap();
        if let Some(t) = tabs.iter_mut().find(|t| t.id == id) {
            t.url = url.clone();
            t.title = "新标签页".into();
            t.webview = Some(label);
        } else {
            tabs.push(TabState { id, url: url.clone(), title: "新标签页".into(), webview: Some(label) });
        }
        *state.active_tab.lock().unwrap() = id;
    }
    let url2 = url.clone();
    tokio::task::block_in_place(move || crate::control::navigate(app, &url2))?;
    // 应用布局:将新标签 webview 定位到页面区全尺寸(预创建时仅在初始 100x100 位置)
    if let Some(win) = app.get_window("main") {
        let size = win.inner_size().map_err(|e| e.to_string())?;
        let scale = win.scale_factor().unwrap_or(1.0);
        apply_layout(app, size, scale).map_err(|e| e.to_string())?;
    }
    emit_tabs_changed(app);
    Ok(())
}

/// 切换标签:隐藏当前 webview,显示目标 webview(不重新加载,保留页面状态)
pub fn switch_tab(app: &AppHandle, id: u32) -> Result<(), String> {
    let state = app.state::<UiState>();
    let active = *state.active_tab.lock().unwrap();
    if active == id {
        return Ok(());
    }
    // 切换标签:退出批注/截图模式(覆盖层停留在旧页面)并自动关闭面板
    exit_modes_and_close_panel(app);
    let (cur_label, target_label) = {
        let tabs = state.tabs.lock().unwrap();
        (
            tabs.iter().find(|t| t.id == active).and_then(|t| t.webview.clone()),
            tabs.iter().find(|t| t.id == id).and_then(|t| t.webview.clone()),
        )
    };
    if let Some(l) = cur_label {
        if let Some(w) = app.get_webview(&l) {
            let _ = w.hide();
        }
    }
    if let Some(l) = target_label {
        if let Some(w) = app.get_webview(&l) {
            w.show().map_err(|e| format!("show webview failed: {e}"))?;
        }
    }
    *state.active_tab.lock().unwrap() = id;
    emit_tabs_changed(app);
    // 切到动态创建的标签时其 webview 会盖住面板,重新置顶浮层
    #[cfg(windows)]
    bring_webviews_to_top(app);
    Ok(())
}

/// 关闭标签:销毁其 webview,切换到邻近标签(关闭最后一个则自动新建空白标签)
pub fn close_tab(app: &AppHandle, id: u32) -> Result<(), String> {
    let state = app.state::<UiState>();
    let mut tabs = state.tabs.lock().unwrap();
    let pos = tabs.iter().position(|t| t.id == id);
    let Some(pos) = pos else { return Err("tab not found".into()) };
    let closed_label = tabs[pos].webview.clone();
    tabs.remove(pos);
    let active = *state.active_tab.lock().unwrap();
    if active != id {
        // 关闭非激活标签:仅移除记录,不切换
        emit_tabs_changed(app);
        return Ok(());
    }
    let new_active;
    if tabs.is_empty() {
        // 关闭最后一个:自动新建空白标签
        let nid = {
            let mut n = state.next_tab_id.lock().unwrap();
            let nid = *n;
            *n += 1;
            nid
        };
        tabs.push(TabState { id: nid, url: NEWTAB_URL.to_string(), title: "新标签页".into(), webview: None });
        new_active = nid;
    } else {
        // 优先激活右侧标签,否则左侧
        new_active = tabs[pos.min(tabs.len() - 1)].id;
    }
    *state.active_tab.lock().unwrap() = new_active;
    let target_label = tabs.iter().find(|t| t.id == new_active).and_then(|t| t.webview.clone());
    drop(tabs);

    // 新建空白标签:优先复用预创建 webview,否则动态创建
    if target_label.is_none() {
        let label = format!("{PAGE_WEBVIEW}-{new_active}");
        let wv = match app.get_webview(&label) {
            Some(w) => w,
            None => create_tab_webview(app, new_active)?,
        };
        wv.show().map_err(|e| format!("show webview failed: {e}"))?;
        {
            let mut tabs = state.tabs.lock().unwrap();
            if let Some(t) = tabs.iter_mut().find(|t| t.id == new_active) {
                t.webview = Some(label);
            }
        }
    }
    // 显示目标 webview(若未隐藏则无操作)
    if let Some(l) = &target_label {
        if let Some(w) = app.get_webview(l) {
            let _ = w.show();
        }
    }
    // 隐藏被关闭标签的 webview(保留实例,避免 z 序变化影响 overlay/panel)
    if let Some(l) = closed_label {
        if let Some(w) = app.get_webview(&l) {
            let _ = w.hide();
        }
    }
    // 新建空白标签导航新标签页
    if target_label.is_none() {
        tokio::task::block_in_place(|| crate::control::navigate(app, NEWTAB_URL))?;
    }
    // 应用布局:确保激活标签 webview 尺寸正确
    if let Some(win) = app.get_window("main") {
        let size = win.inner_size().map_err(|e| e.to_string())?;
        let scale = win.scale_factor().unwrap_or(1.0);
        apply_layout(app, size, scale).map_err(|e| e.to_string())?;
    }
    emit_tabs_changed(app);
    Ok(())
}

/// 规范化地址栏输入:裸域名/主机自动补 http:// 前缀
/// 已含协议(http/https/file/data/about 等含 :// 或专属前缀)则原样返回
pub fn normalize_url(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    if t.contains("://") || t.starts_with("about:") || t.starts_with("data:") || t.starts_with("file:") {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

/// 在激活页面 Webview 执行 JS,返回 JSON 结果(控制协议核心)
pub fn eval_page(app: &AppHandle, js: &str) -> Result<String, String> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
    let page = active_page_webview(app).ok_or("page webview not ready")?;
    page.eval_with_callback(
        js,
        move |result| {
            let _ = tx.send(Ok(result));
        },
    )
    .map_err(|e| format!("eval failed: {e}"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "eval timeout".to_string())?
}

/// 在覆盖层 Webview 执行 JS
pub fn eval_overlay(app: &AppHandle, js: &str) -> Result<(), String> {
    let overlay = overlay_webview(app).ok_or("overlay webview not ready")?;
    overlay.eval(js).map_err(|e| format!("overlay eval failed: {e}"))
}
