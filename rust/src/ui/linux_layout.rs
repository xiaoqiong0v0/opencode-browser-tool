//! Linux 子 webview 绝对定位垫片
//!
//! tauri/wry 在 Linux 上把子 webview `pack_start` 进窗口的 `gtk::Box`(Vertical, expand=true),
//! 导致 tauri 的 `Webview::set_position/set_size`(wry `set_bounds`)静默失效
//! —— wry 只在容器为 `gtk::Fixed`(`is_in_fixed_parent`)时才 `size_allocate`。
//! 本模块用 `webview.with_webview` 拿到 GTK widget(`webkit2gtk::WebView`),自建一个 `gtk::Fixed`
//! 挂到窗口的 vbox,把所有子 webview 重新 `put` 进去,之后用 `Fixed::move_` + `Widget::set_size_request`
//! 实现绝对定位。
//!
//! 注意:GTK 对象非 Send,且所有 GTK 调用都发生在主线程(tauri `with_webview` 在主线程同步执行),
//! 因此用 `thread_local!` 保存 `gtk::Fixed` 与各 webview 最近一次边界。
//!
//! 另:无装饰窗口在 Wayland 下没有可拖拽边框,`install_resize_grips` 在 Fixed 外层套一层
//! `gtk::Overlay` 并叠加 4 条边缘热区 + 4 个角热区,按下时 `begin_resize_drag`
//! (Wayland → xdg_toplevel.resize,X11 → 窗口管理器),两端原生缩放、不依赖 XWayland。
//!
//! `ensure_fixed` 使用自定义子类 `BtFixed`(继承 `gtk::Fixed`)承载 webview:
//! 仅重写 preferred 尺寸 vfunc 使其恒为 1x1,避免子 webview 的 size_request(等于当前窗口尺寸)
//! 反推成容器 minimum size,导致 GTK 拒绝把窗口缩小到当前尺寸以下(只能外拖不能内拖)。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use gtk::prelude::*;
use tauri::Manager;

/// 边缘拖拽缩放热区厚度(物理像素)
const GRIP: i32 = 5;
/// 四角拖拽缩放热区边长(物理像素,略大于 GRIP 便于命中)
const CORNER: i32 = 12;

thread_local! {
    /// 承载所有子 webview 的 gtk::Fixed(GTK 对象非 Send,且仅在主线程访问)
    static FIXED: RefCell<Option<gtk::Fixed>> = RefCell::new(None);
    /// label → 最近一次边界 (x, y, w, h),供 raise 复位用
    static BOUNDS: RefCell<HashMap<String, (i32, i32, i32, i32)>> = RefCell::new(HashMap::new());
    /// 边缘缩放热区是否已安装(幂等标记,重复调用直接返回)
    static GRIPS_INSTALLED: Cell<bool> = Cell::new(false);
}

/// BtFixed 子类的实现体与 GType 注册
///
/// 说明:子 webview 的 `size_request` 恒为当前窗口尺寸,`gtk::Fixed` 默认会据此
/// 把 preferred/minimum 尺寸也报告为当前窗口尺寸;由此 GTK 会拒绝把窗口缩小到
/// 当前尺寸以下(表现为只能外拖不能内拖)。此子类只重写 4 个 preferred 尺寸 vfunc
/// 使其恒返回 1x1,令容器 minimum 与当前尺寸解耦;`size_allocate`(继承 GtkFixed)
/// 仍按子控件的 `move_` 位置 + size_request 正常分配,子 webview 定位不受影响。
mod imp {
    // 注意:glib 不是本 crate 的直接依赖,`#[glib::object_subclass]` 会展开出裸 `glib::`
    // 路径;这里把 gtk 重导出的 glib 引入局部作用域,使展开路径可解析(无需改 Cargo.toml)。
    use gtk::glib;
    use gtk::subclass::prelude::*;

    /// BtFixed 实现体(无额外状态)
    #[derive(Default)]
    pub struct BtFixed;

    #[glib::object_subclass]
    impl ObjectSubclass for BtFixed {
        /// 全局唯一 GType 名
        const NAME: &'static str = "BtFixed";
        /// Rust 包装类型
        type Type = super::BtFixed;
        /// 父类型:沿用 GtkFixed 的布局与子控件管理
        type ParentType = gtk::Fixed;
        /// 不额外实现接口
        type Interfaces = ();
    }

    impl ObjectImpl for BtFixed {}

    impl WidgetImpl for BtFixed {
        /// minimum/natural 宽度恒为 1,解除与子控件 size_request 的关联
        fn preferred_width(&self) -> (i32, i32) {
            (1, 1)
        }

        /// minimum/natural 高度恒为 1,解除与子控件 size_request 的关联
        fn preferred_height(&self) -> (i32, i32) {
            (1, 1)
        }

        /// 指定高度下的 preferred 宽度恒为 1
        fn preferred_width_for_height(&self, _height: i32) -> (i32, i32) {
            (1, 1)
        }

        /// 指定宽度下的 preferred 高度恒为 1
        fn preferred_height_for_width(&self, _width: i32) -> (i32, i32) {
            (1, 1)
        }
    }

    impl ContainerImpl for BtFixed {}

    impl FixedImpl for BtFixed {}
}

gtk::glib::wrapper! {
    /// 自定义 Fixed:preferred 尺寸恒 1x1(实现见 `imp::BtFixed`)
    pub struct BtFixed(ObjectSubclass<imp::BtFixed>)
        @extends gtk::Fixed, gtk::Container, gtk::Widget,
        @implements gtk::Buildable;
}

impl BtFixed {
    /// 创建 BtFixed 实例(preferred 尺寸恒 1x1,其余等同 `gtk::Fixed::new()`)
    fn new() -> Self {
        gtk::glib::Object::new()
    }
}

/// 把指定子 webview 从窗口 GtkBox 迁移到自建 gtk::Fixed(幂等)
/// 参数:app tauri 应用句柄;labels 需要迁移的 webview 标签列表
pub fn reparent(app: &tauri::AppHandle, labels: &[&str]) {
    for label in labels {
        let Some(wv) = app.get_webview(label) else {
            continue;
        };
        let _ = wv.with_webview(|pw| {
            let widget = pw.inner();
            let parent = widget.parent();
            let Some(fixed) = ensure_fixed(&widget) else {
                return;
            };
            // 已在该 Fixed 内:幂等,跳过
            if parent.as_ref() == Some(&fixed.clone().upcast::<gtk::Widget>()) {
                return;
            }
            // 从原父容器(窗口 GtkBox)移除,再 put 进 Fixed
            if let Some(p) = parent {
                if let Ok(container) = p.dynamic_cast::<gtk::Container>() {
                    container.remove(&widget);
                }
            }
            fixed.put(&widget, 0, 0);
            // 初始最小尺寸,随后由 place() 设置真实边界
            widget.set_size_request(1, 1);
        });
    }
}

/// 绝对定位指定子 webview(仅 Linux)
/// 参数:app tauri 应用句柄;label webview 标签;x/y 左上角坐标(GTK 逻辑像素);w/h 尺寸(GTK 逻辑像素,负值按 0 处理)
pub fn place(app: &tauri::AppHandle, label: &str, x: i32, y: i32, w: i32, h: i32) {
    // 先记录边界(供 raise 复位),即使 webview 暂未迁移也不丢坐标
    record_bounds(label, x, y, w, h);
    let Some(wv) = app.get_webview(label) else {
        return;
    };
    let _ = wv.with_webview(move |pw| {
        let widget = pw.inner();
        let Some(fixed) = current_fixed() else {
            return;
        };
        fixed.move_(&widget, x, y);
        widget.set_size_request(w.max(0), h.max(0));
    });
}

/// 将指定子 webview 置顶(从 Fixed 移除后按记录边界重新 put,GTK 子控件顺序即 z 序)
/// 参数:app tauri 应用句柄;label webview 标签
pub fn raise(app: &tauri::AppHandle, label: &str) {
    let Some(wv) = app.get_webview(label) else {
        return;
    };
    let recorded = lookup_bounds(label);
    let _ = wv.with_webview(move |pw| {
        let widget = pw.inner();
        let Some(fixed) = current_fixed() else {
            return;
        };
        fixed.remove(&widget);
        let (x, y, w, h) = match recorded {
            Some(bounds) => bounds,
            // 无记录:用原点与当前 size_request 复位
            None => {
                let (w, h) = widget.size_request();
                (0, 0, w, h)
            }
        };
        fixed.put(&widget, x, y);
        widget.set_size_request(w.max(0), h.max(0));
    });
}

/// 安装窗口边缘拖拽缩放热区(仅 Linux)
///
/// 无装饰窗口(decorations=false)在 Wayland 下没有任何可拖拽边框,窗口无法缩放。
/// 本函数在自建 `gtk::Fixed` 外层再包一个 `gtk::Overlay`,并叠加 4 条边缘 + 4 个角热区;
/// 按下热区时调用 `gtk::Window::begin_resize_drag`,GTK 在 Wayland 转成 xdg_toplevel.resize、
/// 在 X11 交由窗口管理器处理,两端原生可用(不依赖 XWayland)。
///
/// 参数:app tauri 应用句柄;label 一个已迁移到 Fixed 的 webview 标签(取任一即可,如工具栏)
/// 注意:必须在 `reparent` 完成之后调用,否则 `current_fixed` 尚不存在而直接返回
pub fn install_resize_grips(app: &tauri::AppHandle, label: &str) {
    // 幂等:已安装则直接返回
    if GRIPS_INSTALLED.with(|c| c.get()) {
        return;
    }
    let Some(wv) = app.get_webview(label) else {
        return;
    };
    let _ = wv.with_webview(|_pw| {
        // 拿到已迁移的 Fixed;不存在说明 reparent 尚未完成,直接返回
        let Some(fixed) = current_fixed() else {
            return;
        };
        // Fixed 的父容器应为窗口的 GtkBox;已被包进 Overlay 则视为已安装
        let Some(parent) = fixed.parent() else {
            return;
        };
        if parent.clone().dynamic_cast::<gtk::Overlay>().is_ok() {
            GRIPS_INSTALLED.with(|c| c.set(true));
            return;
        }
        let Ok(parent_box) = parent.dynamic_cast::<gtk::Box>() else {
            return;
        };
        // 先把 Fixed 从 GtkBox 摘下,才能挂到 Overlay 下
        parent_box.remove(&fixed);
        // Overlay 包住 Fixed(Fixed 作为主子控件),再整体放回原 GtkBox
        let overlay = gtk::Overlay::new();
        overlay.add(&fixed);
        parent_box.pack_start(&overlay, true, true, 0);
        overlay.show();
        fixed.show();
        // 四边热区:先左右、后上下;后加者在更上层
        add_grip(&overlay, gtk::gdk::WindowEdge::West, gtk::Align::Start, gtk::Align::Fill, GRIP, -1);
        add_grip(&overlay, gtk::gdk::WindowEdge::East, gtk::Align::End, gtk::Align::Fill, GRIP, -1);
        add_grip(&overlay, gtk::gdk::WindowEdge::North, gtk::Align::Fill, gtk::Align::Start, -1, GRIP);
        add_grip(&overlay, gtk::gdk::WindowEdge::South, gtk::Align::Fill, gtk::Align::End, -1, GRIP);
        // 四角热区:必须在四边之后追加,后加者在更上层,四角才优先命中对角拖拽
        // (否则 North/South 为 Fill 且更靠上层,会盖住 West/East,角上只能垂直缩放)
        add_grip(&overlay, gtk::gdk::WindowEdge::NorthWest, gtk::Align::Start, gtk::Align::Start, CORNER, CORNER);
        add_grip(&overlay, gtk::gdk::WindowEdge::NorthEast, gtk::Align::End, gtk::Align::Start, CORNER, CORNER);
        add_grip(&overlay, gtk::gdk::WindowEdge::SouthWest, gtk::Align::Start, gtk::Align::End, CORNER, CORNER);
        add_grip(&overlay, gtk::gdk::WindowEdge::SouthEast, gtk::Align::End, gtk::Align::End, CORNER, CORNER);
        GRIPS_INSTALLED.with(|c| c.set(true));
    });
}

/// 主动触发一次重排+重绘(仅 Linux)
///
/// 参数:app tauri 应用句柄;label webview 标签
/// 说明:WSLg + WebKitGTK 软件渲染(Wayland)下,新 map 的 subsurface 需等输入事件才触发
/// 帧回调/首帧提交,表现为"首次显示面板/覆盖层时不移动鼠标就一直空白"。这里 `queue_resize`
/// + `queue_draw` 令 GTK 主动提交 damage;连同顶层窗口一起 `queue_draw`,确保合成器收到
/// damage 并回帧,从而立即绘制首帧。
pub fn poke(app: &tauri::AppHandle, label: &str) {
    let Some(wv) = app.get_webview(label) else {
        return;
    };
    let _ = wv.with_webview(|pw| {
        let widget = pw.inner();
        widget.queue_resize();
        widget.queue_draw();
        // 连同顶层窗口一起:确保合成器收到 damage 并回帧
        if let Some(top) = widget.toplevel() {
            top.queue_draw();
        }
    });
}

/// 读取窗口的逻辑尺寸(GTK 逻辑像素,即本模块 `move_`/`set_size_request` 所用单位)
///
/// GTK3 的绝对定位 API 以逻辑像素为单位,而 tauri 的 `inner_size()`/`scale_factor()` 是物理/浮点
/// 口径;在非整数缩放(如 GNOME 1.5x 下 GTK 内部取整为 2x)时二者比例与 tao 的 scale_factor 不一致,
/// 直接用物理值会使子视图被放大并裁切。这里直接取该 webview 所属顶层 GtkWindow 的 `allocation()`,
/// 得到与定位 API 完全同口径的逻辑宽高,无需猜测/换算 scale。
///
/// 参数:app tauri 应用句柄;label 任一已创建的 webview 标签(取其顶层窗口)
/// 返回值:Some((width, height)) 逻辑宽高(均 > 0);窗口尚未 map(allocation 为 0)或取不到时 None
pub fn logical_window_size(app: &tauri::AppHandle, label: &str) -> Option<(i32, i32)> {
    let wv = app.get_webview(label)?;
    // with_webview 在 GTK 主线程执行:主线程调用时同步执行(闭包先于 recv 完成);
    // 后台线程调用时经事件循环投递,本线程阻塞等待、不占用主循环(同 screenshot.rs Linux 分支)。
    let (tx, rx) = std::sync::mpsc::channel::<Option<(i32, i32)>>();
    let _ = wv.with_webview(move |pw| {
        let widget = pw.inner();
        let size = widget
            .toplevel()
            .map(|top| {
                let rect = top.allocation();
                (rect.width(), rect.height())
            })
            .filter(|(w, h)| *w > 0 && *h > 0);
        let _ = tx.send(size);
    });
    // 超时兜底:同步路径立即可得;异步路径约一个主循环周期即回。取不到则返回 None 由调用方回退。
    rx.recv_timeout(std::time::Duration::from_millis(500)).ok().flatten()
}

/// 读取当前主线程的 Fixed 句柄
/// 返回值:已创建则 Some(Fixed),否则 None
fn current_fixed() -> Option<gtk::Fixed> {
    FIXED.with(|cell| cell.borrow().clone())
}

/// 记录某 webview 最近一次边界
/// 参数:label webview 标签;x/y/w/h 边界(GTK 逻辑像素)
fn record_bounds(label: &str, x: i32, y: i32, w: i32, h: i32) {
    BOUNDS.with(|m| {
        m.borrow_mut().insert(label.to_string(), (x, y, w, h));
    });
}

/// 读取某 webview 最近一次边界
/// 参数:label webview 标签
/// 返回值:Some((x, y, w, h));从未定位过则 None
fn lookup_bounds(label: &str) -> Option<(i32, i32, i32, i32)> {
    BOUNDS.with(|m| m.borrow().get(label).copied())
}

/// 取得(必要时创建)承载子 webview 的 gtk::Fixed
/// 参数:wv 目标 webview 的 GTK widget
/// 返回值:Some(Fixed);父容器不是 GtkBox/Fixed 时 None
fn ensure_fixed(wv: &webkit2gtk::WebView) -> Option<gtk::Fixed> {
    if let Some(fixed) = current_fixed() {
        return Some(fixed);
    }
    let parent = wv.parent()?;
    // 已是 Fixed(此前已迁移过,仅 thread_local 丢失):直接采用,保证幂等
    if let Ok(fixed) = parent.clone().dynamic_cast::<gtk::Fixed>() {
        FIXED.with(|cell| *cell.borrow_mut() = Some(fixed.clone()));
        return Some(fixed);
    }
    // 父容器为窗口的 GtkBox(tauri/wry 默认):新建 BtFixed 并 pack 进去
    // 用 BtFixed(preferred 恒 1x1)而非 gtk::Fixed,避免窗口 minimum 跟随当前尺寸(只能外拖不能内拖)
    let parent_box = parent.dynamic_cast::<gtk::Box>().ok()?;
    let fixed: gtk::Fixed = BtFixed::new().upcast();
    parent_box.pack_start(&fixed, true, true, 0);
    fixed.show();
    FIXED.with(|cell| *cell.borrow_mut() = Some(fixed.clone()));
    Some(fixed)
}

/// 在 Overlay 上叠加一条边缘拖拽热区
/// 参数:overlay 目标 Overlay;edge 对应的窗口边缘;halign/valign 热区对齐方式;w/h 热区尺寸(负值表示自然尺寸)
/// 按下热区时对顶层窗口发起 `begin_resize_drag`,Wayland(转 xdg_toplevel.resize)与 X11(交 WM)均原生生效
fn add_grip(overlay: &gtk::Overlay, edge: gtk::gdk::WindowEdge, halign: gtk::Align, valign: gtk::Align, w: i32, h: i32) {
    // 用 EventBox 而非 DrawingArea:DrawingArea 没有自己的 GdkWindow,而主子控件
    // WebKitWebView 自带 GdkWindow 且会盖在上面,导致热区收不到按压。
    // EventBox(visible_window=false) 拥有独立 input-only GdkWindow,
    // GtkOverlay 会把它 raise 到主子控件之上,从而正常接收事件。
    let area = gtk::EventBox::new();
    area.set_visible_window(false);
    area.set_above_child(true);
    area.set_events(gtk::gdk::EventMask::BUTTON_PRESS_MASK);
    area.set_halign(halign);
    area.set_valign(valign);
    area.set_size_request(w, h);
    let _ = area.connect_button_press_event(move |widget, event| {
        // 取顶层窗口发起拖拽缩放(坐标/时间戳用于 X11;Wayland 后端按当前事件序列处理)
        if let Some(win) = widget.toplevel().and_then(|t| t.downcast::<gtk::Window>().ok()) {
            win.begin_resize_drag(
                edge,
                event.button() as i32,
                event.root().0 as i32,
                event.root().1 as i32,
                event.time(),
            );
        }
        gtk::glib::Propagation::Proceed
    });
    overlay.add_overlay(&area);
    area.show();
}
