# macOS 随机渲染乱码 + 微信截图右键粘贴 Codex 失败：诊断与日志方案

> 本文档是 **diagnosis-only** 的调试说明，给 Claude / Codex worker 在 `dev` 模式下复现、加临时日志、收集证据用。
>
> 这次先不要盲修；先把两个问题的证据链跑全：
>
> 1. **终端显示随机乱码 / 花屏**：不只中文，英文偶发也会显示异常；但复制出来的文本是正常的。
> 2. **macOS 微信上截图后，右键粘贴进 Codex pane 失败**：同一份剪贴板，用快捷键和右键的表现不一致。

---

## 1. 这次要回答的核心问题

### A. 渲染乱码

先确认它到底属于哪一层：

1. **PTY / 编码层坏了**
   - 如果这层坏，复制出来的文本通常也会是错的。
2. **xterm buffer 正常，但 renderer 画坏了**
   - 如果这层坏，屏幕显示错，但复制出来还是对的。
3. **字体 fallback / 宽度计算出错**
   - 如果这层坏，常见表现是：中文旁边的英文、标点、空格也被“带坏”，看起来像随机乱码。
4. **WebGL renderer 问题**
   - 常见表现：随机、偶发、跟 resize / tab 切换 / 滚动 / 大量输出有关；复制正常，显示异常。

### B. 微信截图右键粘贴失败

先确认失败点在哪一层：

1. **右键事件入口不对**
   - 右键本身先扰动了 Codex TUI，后续 paste 才失败。
2. **剪贴板分类不对**
   - 微信截图本来应被识别成 `raw-image`，结果被归成别的类型。
3. **AI pane 识别不对**
   - pane 实际已经是 Codex 会话，但代码把它当成“非 AI pane”。
4. **provider 路由不对**
   - macOS + codex 本应走 `Alt+V`，结果走成 `Ctrl+V` 或非 AI fallback。
5. **native 图片读取失败**
   - `navigator.clipboard.read()` / `readImage()` / `NSPasteboard` fallback 某一层没拿到图片。

---

## 2. 当前最可疑的代码位置

### 渲染乱码相关

- `src/utils/terminalCache.ts`
  - `getOrCreateTerminal()`
  - `fontFamily`（当前包含 `PingFang SC` / `Hiragino Sans GB`）
  - `Unicode11Addon`
  - `WebglAddon`
- `src/components/TerminalInstance.tsx`
  - mount / fit / resize / refresh 流程
- `src-tauri/src/pty.rs`
  - UTF-8 locale 注入
  - reader flush 的 UTF-8 边界截断

### 右键截图粘贴相关

- `src/components/TerminalInstance.tsx`
  - `handleContextMenu()`
- `src/utils/terminalCache.ts`
  - `getAiProviderForPty()`
  - `detectClipboardPayload()`
  - `sendAiScreenshotImagePaste()`
  - `pasteToTerminal()`
- `src-tauri/src/clipboard.rs`
  - `read_clipboard_image_macos`
  - Finder / pasteboard 相关读取

---

## 3. 日志原则

### 原则 1：先加“可开关”的临时日志，不要常驻刷屏

建议前端统一加一个 helper，例如：

```ts
const TERM_DEBUG = localStorage.getItem('mini-term-debug') === '1';

function debugTerm(scope: string, payload: Record<string, unknown>) {
  if (!TERM_DEBUG) return;
  console.info(`[mini-term-debug] ${scope}`, payload);
}
```

建议后端继续复用已有的：

- `log_perf_from_frontend`
- `read_perf_log`
- `clear_perf_log`

调试前先：

```ts
localStorage.setItem('mini-term-debug', '1');
```

调试结束记得关：

```ts
localStorage.removeItem('mini-term-debug');
```

### 原则 2：日志要能串起来

每条日志尽量至少带：

- `ptyId`
- `provider`
- `paneStatus`
- `platform`
- `ts/perf_now`
- `scope`

这样才能把“右键事件 -> 剪贴板分类 -> paste 分支 -> 最终按键注入 / fallback”串成一条链。

---

## 4. 渲染乱码：建议加的日志

## 4.1 Terminal 创建时的 renderer / 字体日志

文件：`src/utils/terminalCache.ts`

在 `getOrCreateTerminal()` 里加日志：

```ts
debugTerm('terminal:create', {
  ptyId,
  userAgent: navigator.userAgent,
  fontFamily: term.options.fontFamily,
  fontSize: term.options.fontSize,
  lineHeight: term.options.lineHeight,
  letterSpacing: term.options.letterSpacing,
  webglAttempted: true,
});
```

在 WebGL addon load 成功 / 失败 / context loss 时分别打点：

```ts
debugTerm('terminal:webgl_loaded', { ptyId });
debugTerm('terminal:webgl_failed', { ptyId, error: String(err) });
debugTerm('terminal:webgl_context_loss', { ptyId });
```

**目的：**

- 确认渲染异常是否只发生在 `webgl_loaded=true` 的情况下。
- 如果实机一关 WebGL 就好，方向就非常明确。

## 4.2 Resize / mount / refresh 日志

文件：`src/components/TerminalInstance.tsx`

在这些时机加日志：

- mount 完成后第一次 `fit()`
- `ResizeObserver` 触发时
- `theme-changed`
- terminal font size 改动时

日志字段建议：

```ts
debugTerm('terminal:fit', {
  ptyId,
  cols: term.cols,
  rows: term.rows,
  width: container.clientWidth,
  height: container.clientHeight,
  reason: 'initial' | 'resize' | 'font_change' | 'theme_change',
});
```

**目的：**

- 看乱码是否总在某次 `fit()` / resize 后出现。
- 排除“列宽重算错误导致显示错位”。

## 4.3 可切换的 renderer A/B 测试开关

文件：`src/utils/terminalCache.ts`

建议加一个临时开关：

```ts
const FORCE_DISABLE_WEBGL = localStorage.getItem('mini-term-disable-webgl') === '1';
```

如果开关打开，就跳过 `WebglAddon`。

再加一个临时字体开关：

```ts
const FORCE_MONO_ONLY = localStorage.getItem('mini-term-mono-only') === '1';
```

打开时把 `fontFamily` 临时改成纯等宽，例如：

```ts
'JetBrains Mono', 'Cascadia Code', Consolas, monospace
```

**这两个 A/B 开关非常重要：**

- `关 WebGL 后就正常` -> renderer 问题优先
- `关 WebGL 仍异常，但 mono-only 后正常` -> 字体 fallback / glyph 宽度问题优先
- 两者都无效 -> 再回头怀疑更底层的 xterm / refresh / 缓冲问题

## 4.4 渲染问题的证据标准

至少收集这四样：

1. **问题截图**
2. **同一段内容复制出来的真实文本**
3. **是否启用 WebGL**
4. **是否启用 mono-only 字体**

只要出现以下情况，基本就能先把“编码层”排掉：

- 屏幕显示错
- 复制内容正确
- 同一次输出在 buffer 里是正常文本

---

## 5. 微信截图右键粘贴失败：建议加的日志

## 5.1 右键入口日志

文件：`src/components/TerminalInstance.tsx`

在 `handleContextMenu()` 一进来就打：

```ts
debugTerm('contextmenu:enter', {
  ptyId,
  button: e.button,
  selectedText: Boolean(selectedText),
  selectedLength: selectedText.length,
  isMacOS: _isMacOS,
  isWindows: _isWindows,
});
```

右键分流时再打：

```ts
debugTerm('contextmenu:path', {
  ptyId,
  action: selectedText ? 'copy-selected' : 'paste-no-selection',
});
```

**目的：**

- 确认右键确实进入了我们的 handler。
- 确认不是右键事件在更内层被拦截掉了。

## 5.2 AI pane 身份日志

文件：`src/utils/terminalCache.ts`

当前 `pasteToTerminal()` 是这样算的：

- `provider = getAiProviderForPty(ptyId)`
- `isAiPane = !!provider`

这一步很值得怀疑，所以建议把“pane status”和“provider”都打出来。

新增一个只读 helper，返回：

- `paneStatus`
- `aiProvider`
- `isAiByStatus`
- `isAiByProvider`

日志示例：

```ts
debugTerm('paste:ai_identity', {
  ptyId,
  paneStatus,
  provider,
  isAiByStatus,
  isAiByProvider,
});
```

**重点要验证：**

- 是否存在 `paneStatus='ai-awaiting-input'` 或 `ai-complete`，但 `provider=null` 的情况。
- 如果有，而此时又走了非 AI paste 分支，那就是很明确的根因。

## 5.3 剪贴板分类日志

文件：`src/utils/terminalCache.ts`

在 `detectClipboardPayload()` 每个关键分支打点：

```ts
debugTerm('clipboard:web_items', {
  ptyId,
  itemCount: webItems.length,
  itemTypes: webItems.map((i) => i.types),
});
```

```ts
debugTerm('clipboard:classified', {
  ptyId,
  preferImage,
  kind,
  textLen,
  paths,
});
```

失败点也要打：

```ts
debugTerm('clipboard:readImage_failed', { ptyId, error: String(err) });
debugTerm('clipboard:readText_failed', { ptyId, error: String(err) });
debugTerm('clipboard:navigator_read_failed', { ptyId, error: String(err) });
debugTerm('clipboard:macos_native_fallback_failed', { ptyId, error: String(err) });
```

**目的：**

- 看微信截图到底有没有被识别成 `raw-image`。
- 看是 Web Clipboard 没读到，还是 `readImage()` 没读到，还是 macOS native fallback 才能读到。

## 5.4 paste 路由日志

文件：`src/utils/terminalCache.ts`

在 `pasteToTerminal()` 内把最终分支打出来：

```ts
debugTerm('paste:route', {
  ptyId,
  paneStatus,
  provider,
  isAiPane,
  clipboardKind: clipboard.kind,
  route:
    'ai-text' |
    'ai-raw-image' |
    'ai-image-files' |
    'ai-files' |
    'ai-rich-object-ctrl-v' |
    'non-ai-text' |
    'non-ai-image-path' |
    'fallback-alt-v' |
    'noop',
});
```

`sendAiScreenshotImagePaste()` 里再明确打出最终发送的是哪个键：

```ts
debugTerm('paste:image_shortcut', {
  ptyId,
  provider,
  key: 'alt+v' | 'ctrl+v',
  platform: 'mac' | 'windows' | 'linux',
});
```

**目的：**

- 验证 mac + codex + raw-image 时是否真的走了 `Alt+V`。
- 避免“以为走了 Codex 图片分支，实际上走的是普通 fallback”。

## 5.5 后端 perf log 建议

如果前端日志不够，额外用 `log_perf_from_frontend` 打少量关键节点，便于之后统一导出：

- `scope=paste_contextmenu_enter`
- `scope=paste_clipboard_classified`
- `scope=paste_route`
- `scope=render_webgl_loaded`
- `scope=render_webgl_context_loss`

字段建议统一成：

```text
pty_id=3 | provider=codex | pane_status=ai-awaiting-input | kind=raw-image | route=ai-raw-image | key=alt+v
```

---

## 6. dev 模式下的复现脚本

## 6.1 启动前准备

1. 清理旧 perf log
2. 打开前端 debug 开关
3. 启动 `dev`

建议顺序：

```bash
npm run tauri dev
```

打开 DevTools Console，确保能看到 `[mini-term-debug]` 前缀日志。

---

## 6.2 渲染乱码复现脚本

### Case A：纯 shell 高频混合文本输出

在普通 terminal pane 跑：

```bash
python3 - <<'PY'
for i in range(300):
    print(f"{i:03d} | ASCII abcXYZ []() <> -=_+ | 中文测试 渲染检查 | emoji 😀 | Codex Claude Gemini")
PY
```

然后依次做这些动作：

1. 滚动到中间再滚回底部
2. 左右拖动分屏，触发多次 resize
3. 切换主题一次
4. 调大 / 调小 terminal font size
5. 切去别的 tab 再切回来

### Case B：Codex / Claude pane 的长文本输出

让 AI 输出一大段中英混排文本，最好包含：

- 英文单词
- 中文
- 数字
- 标点
- emoji

如果这时屏幕出现花字，但复制出来仍正常，就重点保留当时的 renderer 日志和截图。

### Case C：A/B 对照

同样步骤至少跑三次：

1. 默认配置
2. `localStorage.setItem('mini-term-disable-webgl', '1')`
3. `localStorage.setItem('mini-term-mono-only', '1')`

记录哪一组不再出现问题。

---

## 6.3 微信截图右键粘贴复现脚本

### 主用例：Codex pane

1. 打开一个 Codex pane，确保已经进入可粘贴输入状态。
2. 用微信截图，框选一张图，确认写入系统剪贴板。
3. **先用 `Ctrl+Shift+V` 粘贴一次**，记录是否成功。
4. 同样的流程，再复制一次截图。
5. **再用右键无选中粘贴一次**，记录是否成功。
6. 保存：
   - DevTools console 日志
   - perf log
   - 最终屏幕截图

### 对照用例：Claude pane

同样步骤在 Claude pane 再跑一遍。

### 要重点比较的不是“成不成功”本身，而是：

- 同一份剪贴板，快捷键和右键的 `clipboard.kind` 是否一致
- 同一份剪贴板，快捷键和右键的 `route` 是否一致
- Codex 和 Claude 的 `image_shortcut` 是否按预期分流

---

## 7. 建议的观察结论模板

Claude 跑完后，最好按这个模板给结果：

### 渲染乱码

- 现象：`是否复现`
- 复制内容：`正常 / 不正常`
- WebGL：`开启时复现 / 关闭后消失 / 无影响`
- mono-only 字体：`开启后消失 / 无影响`
- 触发条件：`resize / 滚动 / tab 切换 / AI 长输出 / 随机`
- 初步结论：`renderer` / `字体 fallback` / `仍疑似编码层`

### 微信截图右键粘贴

- 快捷键路径：`成功 / 失败`
- 右键路径：`成功 / 失败`
- `paneStatus`：`...`
- `provider`：`...`
- `isAiByStatus` vs `isAiByProvider`：`...`
- `clipboard.kind`：`...`
- `paste route`：`...`
- `image shortcut`：`alt+v / ctrl+v / none`
- 初步结论：
  - `AI pane 识别错误`
  - `provider 缺失导致走错分支`
  - `clipboard 分类错误`
  - `右键入口先扰动 TUI`
  - `macOS native 图片读取失败`

---

## 8. 判断树：看到什么现象，就优先修什么

### 渲染乱码

#### 如果满足：

- 复制正常
- 关闭 WebGL 后明显不复现

优先结论：

- **先修 WebGL renderer 路径**，不要先去动 PTY 编码层。

#### 如果满足：

- 复制正常
- WebGL 开关无影响
- mono-only 字体后不复现

优先结论：

- **先修 macOS 字体 fallback / monospace 策略**。

#### 如果满足：

- 复制出来也已经错了

优先结论：

- 再回头查 `pty.rs` 的 locale / UTF-8 flush / 原始字节流。

### 微信截图右键粘贴

#### 如果满足：

- `paneStatus` 已经是 AI
- `provider = null`
- `isAiPane = false`

优先结论：

- **`pasteToTerminal()` 的 AI pane 判定逻辑有问题**。

#### 如果满足：

- `clipboard.kind = raw-image`
- provider=codex
- 最终没有发 `Alt+V`

优先结论：

- **mac + codex 图片快捷键路由有问题**。

#### 如果满足：

- 快捷键成功
- 右键失败
- 同一份剪贴板分类相同

优先结论：

- **右键入口本身先扰动了 TUI / 交互路径不等价**。

#### 如果满足：

- `navigator.clipboard.read()` 失败
- `readImage()` 失败
- `read_clipboard_image_macos` 成功

优先结论：

- **微信截图是 macOS 原生 pasteboard 兼容性问题，应保留/增强 native fallback**。

---

## 9. 给 Claude 的执行要求

1. 先只加临时日志和 A/B 开关，不要直接修业务逻辑。
2. 必须实际跑 `dev`，不要只靠静态阅读代码下结论。
3. 两个问题都要保留：
   - 复现步骤
   - 日志片段
   - 截图
   - 初步结论
4. 如果已经能明确缩小到某一个根因，再开第二轮做最小修复。

