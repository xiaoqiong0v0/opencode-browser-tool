import type { LocaleStrings } from "./types.js";

const en: LocaleStrings = {
  tool: {
    "tool.navigate.desc": "Navigate browser to a URL",
    "tool.navigate.arg.url": "Target URL",

    "tool.click.desc": "Click an element on the page",
    "tool.click.arg.selector": "CSS selector for the element",

    "tool.fill.desc": "Fill an input field",
    "tool.fill.arg.selector": "CSS selector for the input field",
    "tool.fill.arg.value": "Value to fill",

    "tool.select.desc": "Select an option from a <select> dropdown",
    "tool.select.arg.selector": "CSS selector for the select element",
    "tool.select.arg.value": "Option value or label to select",

    "tool.hover.desc": "Hover over an element (triggers tooltips, hover effects etc.)",
    "tool.hover.arg.selector": "CSS selector for the element",

    "tool.drag.desc": "Drag an element from source to target",
    "tool.drag.arg.sourceSelector": "CSS selector for the source element",
    "tool.drag.arg.targetSelector": "CSS selector for the target element",

    "tool.press_key.desc": "Press a keyboard key (Enter, Escape, ArrowDown, Tab etc.)",
    "tool.press_key.arg.key":
      "Key name: Enter, Escape, Tab, ArrowDown/Up/Left/Right, Backspace, Delete, a~z, 0~9, F1~F12 etc.",
    "tool.press_key.arg.selector": "Optional CSS selector to focus before pressing",

    "tool.upload_file.desc": "Upload a file to an input[type=file] element",
    "tool.upload_file.arg.selector": "CSS selector for the file input element",
    "tool.upload_file.arg.filePath": "Absolute path to the file",

    "tool.screenshot.desc": "Take a screenshot of the current page or an element, returns base64 image",
    "tool.screenshot.arg.selector": "Optional CSS selector to screenshot a specific element. Omit for full page.",

    "tool.evaluate.desc": "Execute JavaScript in the browser and return the result",
    "tool.evaluate.arg.script": "JavaScript code to execute",

    "tool.get_visible_text.desc": "Get visible text from the current page (helps LLM understand page content)",
    "tool.get_visible_text.arg.selector": "Optional CSS selector to scope text extraction",

    "tool.get_visible_html.desc": "Get HTML structure of the current page (script tags removed by default)",
    "tool.get_visible_html.arg.selector": "Optional CSS selector to scope HTML extraction",
    "tool.get_visible_html.arg.removeScripts": "Remove script tags (default: true)",
    "tool.get_visible_html.arg.removeComments": "Remove HTML comments (default: false)",
    "tool.get_visible_html.arg.maxLength": "Max output length (default: 20000)",

    "tool.console_logs.desc": "Get browser console logs (errors, warnings, info etc.)",
    "tool.console_logs.arg.type": "Filter by type: all, error, warning, log, info, debug (default: all)",
    "tool.console_logs.arg.search": "Search keyword",
    "tool.console_logs.arg.limit": "Max entries (default: 50)",
    "tool.console_logs.arg.clear": "Clear logs after fetching (default: false)",

    "tool.go_back.desc": "Navigate back in browser history",
    "tool.go_forward.desc": "Navigate forward in browser history",

    "tool.resize.desc": "Resize the browser viewport",
    "tool.resize.arg.width": "Viewport width in pixels",
    "tool.resize.arg.height": "Viewport height in pixels",

    "tool.reload.desc": "Reload the current page",
    "tool.clear.desc": "Clear an input field",
    "tool.clear.arg.selector": "CSS selector for the input field",
    "tool.close.desc": "Close the browser (use this when the task is complete)",
    "tool.scroll.desc": "Scroll the page in any direction",
    "tool.scroll.arg.direction": "Direction: up, down, left, right (default: down)",
    "tool.scroll.arg.amount": "Pixels to scroll (default: 300)",
    "tool.wait_for_selector.desc":
      "Wait for an element to appear on the page. Call this before interacting with dynamically loaded elements.",
    "tool.wait_for_selector.arg.selector": "CSS selector to wait for",
    "tool.wait_for_selector.arg.timeout": "Timeout in milliseconds (default: 10000)",
    "tool.click_and_switch_tab.desc": "Click a link and automatically switch to the newly opened tab",
    "tool.click_and_switch_tab.arg.selector": "CSS selector for the link to click",
    "tool.iframe_click.desc": "Click an element inside an iframe",
    "tool.iframe_click.arg.iframeSelector": "CSS selector for the iframe element",
    "tool.iframe_click.arg.selector": "CSS selector for the element inside the iframe",
    "tool.iframe_fill.desc": "Fill an input field inside an iframe",
    "tool.iframe_fill.arg.iframeSelector": "CSS selector for the iframe element",
    "tool.iframe_fill.arg.selector": "CSS selector for the input inside the iframe",
    "tool.iframe_fill.arg.value": "Value to fill",
    "tool.save_as_pdf.desc": "Save the current page as a PDF file",

    "tool.browser_status.desc":
      "Check if the browser is open and what page is currently displayed. Call this whenever you need to understand the current browser state.",

    "tool.list_tabs.desc": "List all open tabs/pages in the current browser session",
    "tool.switch_tab.desc": "Switch to a specific tab by its index number",
    "tool.switch_tab.arg.index": "Tab index number (use bt_list_tabs to see available tabs)",
    "tool.new_tab.desc": "Open a new blank tab or navigate to a URL in a new tab",
    "tool.new_tab.arg.url": "Optional URL to open in the new tab",
    "tool.close_tab.desc": "Close a tab by index, or close the current tab if no index given",
    "tool.close_tab.arg.index": "Optional tab index to close (omit to close current tab)",
    "tool.element_state.desc": "Check if an element exists, is visible, and get its position/size/state on the page",
    "tool.element_state.arg.selector": "CSS selector for the element",

    "tool.scroll_to_element.desc": "Scroll the page until a specific element becomes visible",
    "tool.scroll_to_element.arg.selector": "CSS selector for the element to scroll to",
    "tool.dropdown_options.desc": "List all options in a <select> dropdown element",
    "tool.dropdown_options.arg.selector": "CSS selector for the select element",
    "tool.user_agent.desc": "Set a custom User-Agent string for the browser",
    "tool.user_agent.arg.userAgent": "The User-Agent string to use",

    "tool.select_ua.desc":
      "Select a device preset from Playwright's device registry (207+ devices). Call without 'device' to list all available options.",
    "tool.select_ua.arg.device": "Device name (omit to list available devices)",

    "tool.expect_response.desc":
      "Start waiting for an HTTP response matching a URL pattern. Use bt_assert_response later to check the result.",
    "tool.expect_response.arg.id": "Unique identifier to reference this expectation later",
    "tool.expect_response.arg.url": "URL pattern to match (can be a substring)",
    "tool.assert_response.desc":
      "Check if a previously expected HTTP response has been received. Call this after bt_expect_response.",
    "tool.assert_response.arg.id": "The id used in bt_expect_response",

    "tool.navigate.arg.headless": "Run browser in headless mode (no visible window). Default: false (headed)",
    "tool.show_notification.desc": "Show a notification on the browser page",
    "tool.show_notification.arg.message": "Notification message",
    "tool.show_notification.arg.type": "Type: ok (green), bad (orange), err (red)",

    "tool.accessibility_tree.desc":
      "Get the accessibility tree of the page. More concise than HTML, contains role/name/value semantics, ideal for LLM to understand page structure and locate elements.",
    "tool.accessibility_tree.arg.selector": "Optional CSS selector to scope the tree",
    "tool.accessibility_tree.arg.maxDepth": "Max nesting depth (default: 8)",

    "tool.read_record.desc":
      "Read the full content of an annotation or screenshot record by its ID. Call this when you receive a record reference (id + summary) from the user and need the complete element content (for annotations) or the actual image attachment (for screenshots). Only call this when you actually need the details — the summary (id, tag, annotation) is already provided.",
    "tool.read_record.arg.id": "Record ID number",

    "msg.browser.open": "Browser is open\nTitle: {title}\nURL: {url}\nTabs: {tabs}",
    "msg.browser.closed": "Browser is not open. Use bt_navigate to open a page.",
    "msg.element.not_found": "Element not found: {selector}",
    "msg.select.not_found": "Select element not found: {selector}",
    "msg.navigate.done": "Navigated to: {url}",
    "msg.goback.done": "Went back, current URL: {url}",
    "msg.goforward.done": "Went forward, current URL: {url}",
    "msg.reload.done": "Page reloaded: {url}",
    "msg.press_key.done": "Pressed: {key}",
    "msg.scroll.done": "Scrolled {dir} by {amount}px",
    "msg.tab.switched": "Switched to tab #{idx}: {url}",
    "msg.tab.new": "New tab opened: {url}",
    "msg.tab.closed": "Closed tab #{idx}",
    "msg.tab.closed_current": "Closed current tab",
    "msg.click_switch.done": "Clicked and switched to new tab: {url}",
    "msg.click_switch.clicked": "Clicked: {selector}",
    "msg.wait_selector.found": "Element appeared",
    "msg.record.sent": " (sent)",
    "msg.browser.installing": "Installing {browser} ({progress}). Please try bt_navigate again in a moment.",
  },

  panel: {
    "panel.title": "Tool Panel",
    "panel.pick_hint": "Hover to highlight elements\nClick to select for annotation",
    "panel.annotation_placeholder": "Add annotation...",
    "panel.action_none": "No action",
    "panel.action_click": "Click",
    "panel.action_fill": "Fill",
    "panel.action_value_placeholder": "Value...",
    "panel.send": "Send",
    "panel.send_all": "Send All",
    "panel.close": "Close",
    "panel.send_failed": "Send failed",
    "panel.annotate": "Annotate",
    "panel.screenshot_btn": "Screenshot",
    "panel.screenshot_sent": "Screenshot sent",
    "panel.screenshot_note": "Add description...",
    "panel.screenshot_hint": "Double click to select entire page",
    "panel.screenshot_stored": "Screenshot saved",
    "panel.screenshot_failed": "Screenshot failed",
    "panel.cancel": "Cancel",
    "panel.confirm": "Confirm",
    "panel.annotation_stored": "Annotation saved",
    "panel.annotation_required": "Please enter annotation text",
    "panel.no_records": "No records yet",
    "panel.sent_records": "Sent {n} records",
    "panel.send_records_failed": "Send failed",
    "panel.delete": "Delete",
  },
};

export default en;
