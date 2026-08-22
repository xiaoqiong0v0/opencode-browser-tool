import type { LocaleStrings } from "./types.js";

const zh: LocaleStrings = {
  tool: {
    "tool.navigate.desc": "导航浏览器到指定 URL",
    "tool.navigate.arg.url": "目标 URL",
    "tool.navigate.arg.headless": "无头模式运行（不显示窗口）。默认 false（有界面）",

    "tool.click.desc": "点击页面元素",
    "tool.click.arg.selector": "元素的 CSS 选择器",

    "tool.fill.desc": "填写输入框",
    "tool.fill.arg.selector": "输入框的 CSS 选择器",
    "tool.fill.arg.value": "要填写的值",

    "tool.select.desc": "选择下拉框选项",
    "tool.select.arg.selector": "select 元素的 CSS 选择器",
    "tool.select.arg.value": "要选择的 option 值或标签",

    "tool.hover.desc": "鼠标悬停在元素上（触发 tooltip 等效果）",
    "tool.hover.arg.selector": "元素的 CSS 选择器",

    "tool.drag.desc": "拖拽元素从源位置到目标位置",
    "tool.drag.arg.sourceSelector": "源元素的 CSS 选择器",
    "tool.drag.arg.targetSelector": "目标元素的 CSS 选择器",

    "tool.press_key.desc": "按下键盘按键（Enter、Escape、方向键等）",
    "tool.press_key.arg.key":
      "按键名: Enter, Escape, Tab, ArrowDown/Up/Left/Right, Backspace, Delete, a~z, 0~9, F1~F12 等",
    "tool.press_key.arg.selector": "可选，先聚焦到该元素再按键",

    "tool.upload_file.desc": "上传文件到 input[type=file] 元素",
    "tool.upload_file.arg.selector": "文件输入框的 CSS 选择器",
    "tool.upload_file.arg.filePath": "文件的绝对路径",

    "tool.screenshot.desc": "对当前页面或指定元素截图，返回 base64 图片",
    "tool.screenshot.arg.selector": "可选，CSS 选择器对指定元素截图。不传则截全页",

    "tool.evaluate.desc": "在浏览器中执行 JavaScript 代码并返回结果",
    "tool.evaluate.arg.script": "要执行的 JavaScript 代码",

    "tool.get_visible_text.desc": "获取当前页面可见文本（帮助 LLM 理解页面内容）",
    "tool.get_visible_text.arg.selector": "可选，限定到指定元素内的文本",

    "tool.get_visible_html.desc": "获取当前页面的 HTML 结构（默认移除 script 标签）",
    "tool.get_visible_html.arg.selector": "可选，限定到指定元素内的 HTML",
    "tool.get_visible_html.arg.removeScripts": "是否移除 script 标签（默认: true）",
    "tool.get_visible_html.arg.removeComments": "是否移除注释（默认: false）",
    "tool.get_visible_html.arg.maxLength": "最大返回字符数（默认: 20000）",

    "tool.console_logs.desc": "获取浏览器控制台日志（错误、警告、info 等）",
    "tool.console_logs.arg.type": "过滤类型: all, error, warning, log, info, debug（默认 all）",
    "tool.console_logs.arg.search": "搜索关键词",
    "tool.console_logs.arg.limit": "返回条数上限（默认 50）",
    "tool.console_logs.arg.clear": "获取后是否清空日志（默认 false）",

    "tool.go_back.desc": "浏览器后退到上一页",
    "tool.go_forward.desc": "浏览器前进到下一页",

    "tool.resize.desc": "调整浏览器视口尺寸",
    "tool.resize.arg.width": "视口宽度（像素）",
    "tool.resize.arg.height": "视口高度（像素）",

    "tool.set_device.desc":
      "应用设备预设（窗口尺寸 + User-Agent）。不传 name 参数可列出所有可用预设。",
    "tool.set_device.arg.name": "设备预设名称（不传则列出可用预设）",

    "tool.devtools.desc": "打开/关闭开发者工具（F12）",
    "tool.devtools.arg.action": "操作: open, close, toggle（默认 toggle）",

    "tool.reload.desc": "刷新当前页面",
    "tool.clear.desc": "清空输入框",
    "tool.clear.arg.selector": "输入框的 CSS 选择器",
    "tool.close.desc": "关闭浏览器（任务完成时调用）",
    "tool.scroll.desc": "滚动页面方向",
    "tool.scroll.arg.direction": "方向: up, down, left, right（默认 down）",
    "tool.scroll.arg.amount": "滚动像素（默认 300）",
    "tool.wait_for_selector.desc": "等待元素出现在页面上。在操作动态加载的元素前调用此工具。",
    "tool.wait_for_selector.arg.selector": "等待的 CSS 选择器",
    "tool.wait_for_selector.arg.timeout": "超时毫秒（默认 10000）",
    "tool.click_and_switch_tab.desc": "点击链接并自动切换到新打开的标签页",
    "tool.click_and_switch_tab.arg.selector": "链接的 CSS 选择器",
    "tool.iframe_click.desc": "在 iframe 中点击元素",
    "tool.iframe_click.arg.iframeSelector": "iframe 的 CSS 选择器",
    "tool.iframe_click.arg.selector": "iframe 内元素的 CSS 选择器",
    "tool.iframe_fill.desc": "在 iframe 中填写输入框",
    "tool.iframe_fill.arg.iframeSelector": "iframe 的 CSS 选择器",
    "tool.iframe_fill.arg.selector": "iframe 内输入框的 CSS 选择器",
    "tool.iframe_fill.arg.value": "要填写的值",
    "tool.save_as_pdf.desc": "将当前页面保存为 PDF",

    "tool.browser_status.desc": "检查浏览器是否已打开以及当前显示的页面。当你需要了解当前浏览器状态时调用此工具。",

    "tool.list_tabs.desc": "列出当前浏览器会话中的所有标签页",
    "tool.switch_tab.desc": "按索引切换到指定标签页",
    "tool.switch_tab.arg.index": "标签页索引（用 bt_list_tabs 查看可用标签页）",
    "tool.new_tab.desc": "新建空白标签页或在新的标签页中打开指定 URL",
    "tool.new_tab.arg.url": "可选，在新标签页中打开的 URL",
    "tool.close_tab.desc": "关闭指定索引的标签页，不传索引则关闭当前标签页",
    "tool.close_tab.arg.index": "可选，要关闭的标签页索引",
    "tool.element_state.desc": "检查元素是否存在、可见、获取位置/大小/状态",
    "tool.element_state.arg.selector": "元素的 CSS 选择器",

    "tool.scroll_to_element.desc": "滚动页面直到指定元素可见",
    "tool.scroll_to_element.arg.selector": "要滚动到的元素 CSS 选择器",
    "tool.dropdown_options.desc": "列出 <select> 下拉框的所有选项",
    "tool.dropdown_options.arg.selector": "select 元素的 CSS 选择器",
    "tool.user_agent.desc": "设置自定义 User-Agent",
    "tool.user_agent.arg.userAgent": "要使用的 User-Agent 字符串",

    "tool.select_ua.desc":
      "从内置设备列表中选择设备预设。不传 device 参数可列出所有可用选项。",
    "tool.select_ua.arg.device": "设备名称（不传则列出可用设备）",

    "tool.expect_response.desc": "开始等待匹配 URL 模式的 HTTP 响应。之后用 bt_assert_response 检查结果。",
    "tool.expect_response.arg.id": "唯一标识符，用于后续检查",
    "tool.expect_response.arg.url": "要匹配的 URL 模式（支持子串匹配）",
    "tool.assert_response.desc": "检查之前等待的 HTTP 响应是否已收到。需在 bt_expect_response 之后调用。",
    "tool.assert_response.arg.id": "bt_expect_response 中使用的 id",

    "tool.show_notification.desc": "在浏览器页面显示通知",
    "tool.show_notification.arg.message": "通知内容",
    "tool.show_notification.arg.type": "类型: ok（绿色）, bad（橙色）, err（红色）",

    "tool.accessibility_tree.desc":
      "获取页面可访问性树，比 HTML 更简洁，包含 role/name/value 语义，适合 LLM 理解页面结构和定位元素",
    "tool.accessibility_tree.arg.selector": "可选，限定到指定元素子树",
    "tool.accessibility_tree.arg.maxDepth": "最大嵌套深度（默认 8）",

    "tool.read_record.desc": "按 id 读取批注或截图记录的完整内容。批注返回元素内容，截图返回图片附件。",
    "tool.read_record.arg.id": "记录 ID",

    "msg.browser.open": "浏览器已打开\n标题: {title}\nURL: {url}\n标签页: {tabs}",
    "msg.browser.closed": "浏览器未打开，请先使用 bt_navigate 打开页面",
    "msg.element.not_found": "未找到元素: {selector}",
    "msg.select.not_found": "未找到下拉框: {selector}",
    "msg.navigate.done": "已导航到: {url}",
    "msg.goback.done": "已返回，当前 URL: {url}",
    "msg.goforward.done": "已前进，当前 URL: {url}",
    "msg.reload.done": "页面已刷新: {url}",
    "msg.press_key.done": "已按键: {key}",
    "msg.scroll.done": "已滚动 {dir} {amount}px",
    "msg.tab.switched": "已切换到标签页 #{idx}: {url}",
    "msg.tab.new": "已打开新标签页: {url}",
    "msg.tab.closed": "已关闭标签页 #{idx}",
    "msg.tab.closed_current": "已关闭当前标签页",
    "msg.click_switch.done": "已点击并切换到新标签页: {url}",
    "msg.click_switch.clicked": "已点击: {selector}",
    "msg.wait_selector.found": "元素已出现",
    "msg.record.sent": " (已发送)",
    "msg.browser.installing": "正在安装 {browser}（{progress}）。请稍后重试 bt_navigate。",
    "msg.device.set": "已应用设备预设: {name} ({size})\nUser-Agent: {ua}",
    "msg.devtools.open": "开发者工具已打开",
    "msg.devtools.closed": "开发者工具已关闭",
  },

  panel: {
    "panel.title": "工具面板",
    "panel.pick_hint": "悬停高亮页面元素\n点击选中进行标注",
    "panel.annotation_placeholder": "添加批注...",
    "panel.action_none": "无操作",
    "panel.action_click": "点击",
    "panel.action_fill": "填写",
    "panel.action_value_placeholder": "值...",
    "panel.send": "发送",
    "panel.send_all": "发送全部",
    "panel.close": "关闭",
    "panel.send_failed": "发送失败",
    "panel.annotate": "批注",
    "panel.screenshot_btn": "截图",
    "panel.screenshot_sent": "截图已发送",
    "panel.screenshot_note": "添加说明...",
    "panel.screenshot_hint": "双击截取全屏",
    "panel.screenshot_stored": "截图已保存",
    "panel.screenshot_failed": "截图失败",
    "panel.cancel": "取消",
    "panel.confirm": "确认",
    "panel.annotation_required": "请输入批注文字",
    "panel.annotation_stored": "批注已保存",
    "panel.no_records": "当前记录为空",
    "panel.sent_records": "已发送 {n} 条记录",
    "panel.send_records_failed": "发送失败",
    "panel.delete": "删除",
  },
};

export default zh;
