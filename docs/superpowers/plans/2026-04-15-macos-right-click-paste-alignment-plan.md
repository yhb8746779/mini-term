# macOS 右键粘贴对齐 Windows 现状的实现规划（只改 mac，不动 Windows）

> 适用基线：`main` 当前最新提交 `1cc592a`
>
> 这份文档只解决 **macOS** 侧和最新 Windows 行为不一致的问题，目标是让 Claude Code / Codex 在 mac 上尽量贴近你已经在 Windows 上验证过的三分支效果：
>
> 1. 截图粘贴 -> AI 侧展示成 `[Image #n]`
> 2. 文本粘贴 -> AI 侧展示成 `[Pasted text #n]`
> 3. 文件粘贴 -> Claude Code 自己判断图片/文件；图片文件走图片块，普通文件走路径/文件引用
>
> **重要约束：本方案只动 mac 逻辑，不改 Windows 已经跑通的分支。**

---

## 1. 先说结论

当前代码里，Windows 之所以已经能分成 3 条路径，是因为它已经具备：

- 截图位图：`raw-image`
- Explorer 图片文件/普通文件：`explorer-image-files` / `explorer-files`
- 纯文本：`plain-text`

而 mac 现在的问题，不是右键入口本身，而是 **`pasteToTerminal()` 里的 mac 分类和 mac native 路径还没补齐**。

也就是说：

- `src/components/TerminalInstance.tsx` 里的 mac 右键“有选中复制 / 无选中粘贴”已经是对的
- 真正缺的是 `src/utils/terminalCache.ts` 的 mac 分流，和 `src-tauri/src/clipboard.rs` 的 mac 文件路径读取能力

---

## 2. 当前 mac 出问题的根因

## 2.1 截图：Claude Code 好使，Codex 不好使

当前 `src/utils/terminalCache.ts:381` 的 `sendAiScreenshotImagePaste()` 是：

- Windows -> `Alt+V`
- macOS/Linux -> `Ctrl+V`

也就是现在 mac 对所有 provider 都统一发 `Ctrl+V`。

这和你现在的实测不一致：

- **Claude Code on mac**：右键截图粘贴是通的
- **Codex on mac**：右键截图粘贴不通

这说明：

- mac 的 `raw-image` 分支，不能继续所有 provider 一刀切 `Ctrl+V`
- 至少 **Codex on mac** 应该单独走另一条快捷键分支

结合你之前多轮实测，当前最合理的落法是：

- mac + `claude` -> `Ctrl+V`
- mac + `codex` -> `Alt+V`
- mac + `gemini` -> 第一轮先跟 `claude` 保持 `Ctrl+V`

这里要强调：

- 这是基于你当前最新实测得到的工程结论
- 不是要把 Windows 再改回 provider 分流
- 只在 **mac raw-image** 这一条分支做 provider 细分

## 2.2 文本：右键粘贴后的样式不是 `[Pasted text #n]`

当前 AI 文本路径其实已经有正确实现：

- `src/utils/terminalCache.ts:363` `sendAiTextPaste()`
- 它调用 `term.paste(text)`
- 会触发 bracketed paste，被 Claude/Codex 识别成 `[Pasted text #n]`

所以问题不在“怎么发文本”，而在“文本有没有被正确分类成 `plain-text`”。

当前 `detectClipboardPayload()` 在 `src/utils/terminalCache.ts:503` 这一段里，逻辑是：

- 只要 `ClipboardItem.types` 里出现了不是 `text/plain` / `text/html` 的类型
- 就直接归类成 `rich-object`

这在 Windows 上问题不大，但在 mac 上很容易把“其实就是普通文本”的内容误判成 `rich-object`。原因是很多 mac 应用复制文本时，会同时带这些伴随类型：

- `text/rtf`
- `public.utf8-plain-text`
- `public.rtf`
- `public.html`

一旦被误判成 `rich-object`，AI pane 当前就不会走 `term.paste(text)`，而是落到：

- `src/utils/terminalCache.ts:588` -> 直接发 `Ctrl+V`

这样最终表现就不是 `[Pasted text #n]` 那条路径了。

所以 mac 文本样式不对，本质上是：

- **分类错了，不是发送错了**

## 2.3 文件：mac 上完全粘贴不进去

这个问题最明确。

当前 Windows 能处理 Explorer 文件粘贴，是因为有：

- `src-tauri/src/clipboard.rs:452` `read_clipboard_file_paths()`
- 它读取 Win32 `CF_HDROP`
- 前端 `src/utils/terminalCache.ts:490` 再把它分成：
  - `explorer-image-files`
  - `explorer-files`

但 mac 当前完全没有对等能力：

- `src-tauri/src/clipboard.rs:315` 之后的 mac 模块只实现了图片读取
- 没有 `read_clipboard_file_paths_macos()` 之类的命令
- `src/utils/terminalCache.ts:490` 的 Step 2 又只在 `_isWindows` 时执行

所以 Finder 复制文件后，mac 这边会发生的事情通常是：

1. 前端拿不到可用 `text/plain` 全路径
2. 也拿不到 `raw-image`
3. 最后被归进 `rich-object` / `empty-or-unknown`
4. AI pane 再走 `Ctrl+V` fallback

这也是为什么你看到“文件完全粘贴不进去”。

---

## 3. 当前 Windows 已有能力，mac 需要补哪几块

为了确保 **不影响 Windows**，这轮不要重构 Windows 那套分类名和路径，只在 mac 旁边补分支。

建议补 3 块：

1. **mac raw-image provider 分流**
2. **mac text-like 类型放宽，不要误判 rich-object**
3. **mac Finder 文件路径原生读取**

---

## 4. 具体改造方案

## Task 1：只在 mac 的截图分支恢复 provider 细分

**文件：** `src/utils/terminalCache.ts`

### 目标

只修这一件事：

- `raw-image` 在 mac 上不要一律 `Ctrl+V`
- 保留 Windows 现在的 `Alt+V` 不变

### 推荐做法

把现在的：

```ts
async function sendAiScreenshotImagePaste(ptyId: number): Promise<void> {
  if (_isWindows) {
    await enqueuePtyWrite(ptyId, '\x1bv');
  } else {
    await enqueuePtyWrite(ptyId, '\x16');
  }
}
```

改成“平台主分流 + mac 局部 provider 分流”：

```ts
async function sendAiScreenshotImagePaste(
  ptyId: number,
  provider: AiProvider | null,
): Promise<void> {
  if (_isWindows) {
    await enqueuePtyWrite(ptyId, '\x1bv');
    return;
  }

  if (_isMacOS) {
    if (provider === 'codex') {
      await enqueuePtyWrite(ptyId, '\x1bv');
      return;
    }
    await enqueuePtyWrite(ptyId, '\x16');
    return;
  }

  await enqueuePtyWrite(ptyId, '\x16');
}
```

然后在 `pasteToTerminal()` 里调用时，把 provider 传进去。

### 为什么这样改

- 你现在的实测已经证明：mac 上 Claude 和 Codex 对 raw-image 不是同一个快捷键
- 但这个差异目前只体现在 mac raw-image 分支
- **Windows 不要动**，继续保持你刚优化好的三分支逻辑

### 注意

- 不要把 Windows 的 `explorer-image-files` / `explorer-files` 逻辑一起重构
- 只改 `raw-image` 这一条 mac 分支

---

## Task 2：只在 mac 放宽“文本型 clipboard item”的识别规则

**文件：** `src/utils/terminalCache.ts`

### 当前问题点

当前这段判断过于激进：

```ts
const hasRich = item.types.some((t) => t !== 'text/plain' && t !== 'text/html');
if (hasRich) {
  return { kind: 'rich-object' };
}
```

在 mac 上，这会把很多本来应该走 `plain-text` 的内容误判成 `rich-object`。

### 推荐做法

新增一个 mac-only helper：

```ts
const MAC_TEXT_LIKE_TYPES = new Set([
  'text/plain',
  'text/html',
  'text/rtf',
  'public.utf8-plain-text',
  'public.html',
  'public.rtf',
]);

function isMacTextLikeClipboardItem(item: ClipboardItem): boolean {
  return item.types.every((t) => MAC_TEXT_LIKE_TYPES.has(t));
}
```

然后在 `detectClipboardPayload()` 的 Web API 解析里改成：

- Windows：保持现在逻辑不动
- mac：
  - 如果 item 全部都是 text-like 类型，就优先尝试读文本
  - 只有出现真正文件 / 图片 / 非文本对象时，才归类成 `rich-object`

### 推荐顺序

在 mac 下改成：

1. 先识别 `image/*` -> `raw-image`
2. 再识别 Finder 文件对象（Task 3）
3. 再识别 text-like item -> `plain-text`
4. 剩余的才归 `rich-object`

### 为什么这样改

因为你现在的“文本样式不对”，本质就是文本没走到 `sendAiTextPaste()`。

一旦分类修正，AI pane 文本就会重新走：

- `term.paste(text)`
- 然后回到 `[Pasted text #n]`

### 注意

- 这块一定要 `if (_isMacOS)` 局部处理
- 不要把 Windows 的 `ClipboardItem.types` 策略一起改掉，免得把刚修好的 Explorer 路径识别又搅乱

---

## Task 3：给 mac 补 Finder 文件路径读取命令

**文件：**

- `src-tauri/src/clipboard.rs`
- `src-tauri/src/lib.rs`
- `src/utils/terminalCache.ts`

### 目标

让 mac 也具备和 Windows `CF_HDROP` 对等的“原生文件路径提取”能力。

### 后端建议

在 `src-tauri/src/clipboard.rs` 增加一个新命令，例如：

```rust
#[tauri::command]
pub fn read_clipboard_file_paths_macos() -> Result<Vec<String>, String>
```

实现建议：

- 仍然基于 `NSPasteboard`
- 优先尝试读取 file URL / NSURL 对象
- 过滤 `isFileURL == true`
- 转成标准本地路径字符串数组返回

实现思路优先级：

### 方案 A：直接读 `NSURL`

优先推荐：

- `NSPasteboard` + `readObjectsForClasses:options:`
- classes 传 `NSURL`
- 遍历结果，只保留 `fileURL`
- 最后拿 `.path`

这条路线的好处是：

- 不需要自己去解析 `public.file-url` 的原始数据
- 更接近 Finder 原生文件粘贴语义
- 也更适合多文件场景

### 方案 B：按 pasteboard type 读 file-url

如果 `objc2` 绑定写起来不顺，可以退一步：

- 检查 `public.file-url`
- 或其他 file url 相关 UTI
- 把 URL 字符串 decode 成本地路径

但优先仍建议方案 A。

### 前端建议

在 `src/utils/terminalCache.ts` 里，保留 Windows 现有 Step 2 不动，再新增一个 mac-only Step 2b：

```ts
if (_isMacOS) {
  try {
    const paths = await invoke<string[]>('read_clipboard_file_paths_macos');
    if (paths.length > 0) {
      const allImages = paths.every(isImageFilePath);
      return allImages
        ? { kind: 'finder-image-files', paths }
        : { kind: 'finder-files', paths };
    }
  } catch {}
}
```

### 为什么建议新增 `finder-image-files` / `finder-files`

而不是直接复用 Windows 那两个 kind 名字：

- `explorer-image-files`
- `explorer-files`

原因很简单：

- 你这轮要求“只动 mac，不影响 Windows”
- 新增 mac kind，比重命名现有 Windows kind 风险更低
- 逻辑更直观，review 时也更容易看出只是在补 mac

### AI pane 里的处理

在 `pasteToTerminal()` 中新增：

```ts
if ((clipboard.kind === 'finder-image-files' || clipboard.kind === 'finder-files') && clipboard.paths) {
  sendAiExplorerFilesPaste(ptyId, clipboard.paths);
  return;
}
```

这里虽然 helper 还叫 `sendAiExplorerFilesPaste()`，但第一轮可以先不改名字，避免不必要 churn。

### 为什么这样能满足你的目标

因为 Claude Code 对“图片文件 / 普通文件”的处理，本质不是靠图片位图快捷键，而是靠：

- 收到文件路径文本
- 再自己判断是图片还是文件

这和你 Windows 上现在的第三条分支是一致的。

也就是说，mac 文件粘贴要补的不是 `Alt+V` / `Ctrl+V`，而是：

- **先把 Finder 复制的文件路径拿出来**
- **再按路径文本送进 AI pane**

---

## Task 4：`pasteToTerminal()` 只补 mac 分支，不动 Windows 现状

**文件：** `src/utils/terminalCache.ts`

### 推荐改法

#### Step 1：拿 provider 但只给 mac raw-image 用

```ts
const provider = getAiProviderForPty(ptyId);
const isAiPane = !!provider;
```

#### Step 2：AI pane 路径保持 Windows 现状，mac 只补缺口

推荐 AI 分支最终结构：

```ts
if (isAiPane) {
  if (clipboard.kind === 'raw-image') {
    await sendAiScreenshotImagePaste(ptyId, provider);
    return;
  }

  if (clipboard.kind === 'plain-text' && clipboard.text) {
    sendAiTextPaste(ptyId, clipboard.text);
    return;
  }

  if (
    (clipboard.kind === 'explorer-image-files' || clipboard.kind === 'explorer-files' ||
     clipboard.kind === 'finder-image-files' || clipboard.kind === 'finder-files') &&
    clipboard.paths
  ) {
    sendAiExplorerFilesPaste(ptyId, clipboard.paths);
    return;
  }

  if (clipboard.kind === 'rich-object') {
    // Windows 保持现在逻辑
    // mac 第一轮也先保守 fallback，但不要覆盖上面三条已识别路径
    await enqueuePtyWrite(ptyId, '\x16');
    return;
  }

  if (clipboard.text) {
    sendAiTextPaste(ptyId, clipboard.text);
  }
  return;
}
```

### 核心原则

- Windows 现有分支顺序不要改
- 只是给 mac 补：
  - raw-image provider 差异
  - Finder 文件路径
  - text-like 修正

---

## 5. 为什么这样能和你 Windows 目标对齐

你现在 Windows 侧已经验证出来的三条路，本质是：

### 路 1：截图 / 原始图片位图

- 目标是出现 `[Image #n]`
- 这条路不靠路径文本，而靠 AI CLI 自己读系统剪贴板图片

### 路 2：纯文本

- 目标是出现 `[Pasted text #n]`
- 这条路应该走 `term.paste(text)`，不是直接注入普通字符流

### 路 3：复制的文件

- 目标是 Claude Code 自己判断图片文件 or 普通文件
- 这条路靠的是“路径文本注入”，不是 raw-image 快捷键

mac 现在正好缺的就是：

- 路 1：Codex 和 Claude 没分开
- 路 2：文本常常被误判成 rich-object
- 路 3：Finder 文件路径根本没提取出来

所以补完这 3 件事，mac 就能和 Windows 保持同一套语义，只是实现细节按平台分流。

---

## 6. 明确哪些地方不要动

为了确保不影响 Windows，这轮 **不要** 动这些地方：

1. 不要改 `src/utils/terminalCache.ts:490` 现有 `_isWindows` 的 `read_clipboard_file_paths` 分支顺序
2. 不要把 `explorer-image-files` / `explorer-files` 重命名成通用名字
3. 不要改 Windows 的 `Alt+V` 截图路径
4. 不要把 Linux 也顺手改了
5. 不要把 `TerminalInstance.tsx` 的右键入口再改回菜单

---

## 7. 推荐验证矩阵（只测 mac）

## 7.1 Claude Code on mac

### 截图

- [ ] 微信截图确认后，右键无选中
- 预期：显示成 `[Image #n]`

### 文本

- [ ] 从 Notes / 浏览器 / 微信复制一段普通文本后右键
- 预期：显示成 `[Pasted text #n]`
- 注意：这一步要覆盖“带 RTF 的文本来源”

### 文件

- [ ] Finder 复制一张 png/jpg 后右键
- 预期：Claude Code 识别成图片输入
- [ ] Finder 复制一个普通文件后右键
- 预期：Claude Code 识别成文件 / 路径引用

## 7.2 Codex on mac

### 截图

- [ ] 微信截图确认后，右键无选中
- 预期：`raw-image` 改走 mac+codex 专属分支后恢复可用

### 文本

- [ ] 普通文本右键
- 预期：走 bracketed paste，不再退成 fallback 粘贴

### 文件

- [ ] Finder 复制图片文件 / 普通文件后右键
- 预期：至少先能稳定把路径注入进去，不再“完全贴不进去”

---

## 8. 建议提交拆分

为了 review 和回滚都简单，建议分 3 个 commit：

### Commit 1

```bash
fix(mac-clipboard): restore provider-aware raw-image paste for codex on macOS
```

只改：

- `src/utils/terminalCache.ts`
- 只动 mac raw-image 分支

### Commit 2

```bash
fix(mac-clipboard): treat text-like clipboard items as plain text on macOS
```

只改：

- `src/utils/terminalCache.ts`
- 只动 mac text classification

### Commit 3

```bash
feat(mac-clipboard): support Finder file path paste for AI panes
```

改：

- `src-tauri/src/clipboard.rs`
- `src-tauri/src/lib.rs`
- `src/utils/terminalCache.ts`

---

## 9. 完成定义

下面这些都满足，才算 mac 这轮补齐完成：

- mac 右键截图：Claude Code 可用，Codex 也恢复可用
- mac 右键文本：重新走 `[Pasted text #n]`
- mac Finder 复制图片文件：可粘贴进 AI pane
- mac Finder 复制普通文件：也可粘贴进 AI pane
- Windows 当前三分支行为完全不回退
- `npm run build` 通过
- 如有 Rust 环境，再补 `cargo check`

---

## 10. 参考

下面两个官方文档可以作为实现 mac 原生文件路径读取时的依据：

- Apple `NSPasteboard.PasteboardType.fileURL`:
  https://developer.apple.com/documentation/appkit/nspasteboard/pasteboardtype/fileurl
- Apple `NSPasteboard` / `readObjectsForClasses` 相关文档:
  https://developer.apple.com/documentation/appkit/nspasteboard
