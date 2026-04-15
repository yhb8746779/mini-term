# AI 图片粘贴优先级 Follow-up 计划

> **For Claude Code / agentic workers:** 当前实现会优先把剪贴板图片保存为临时 PNG，再把“文件路径”写进 PTY。这个行为对通用终端是安全的，但对 Claude / Codex 这类已经支持“图片附件粘贴”的 AI TUI 来说，体验不对：用户期望看到 `image #1` / `image1` 这类附件占位，而不是一串 temp 文件路径。  
> 本文档的目标是：**在 AI TUI 中优先恢复原生图片粘贴体验**，只有失败时才退回路径方案。

## 背景

当前图片粘贴相关提交：

- `194c2e3` - `feat(clipboard): 图片粘贴支持 + macOS Ctrl 键修复`
- `57cb3fd` - `fix(clipboard): 补齐 macOS 剪贴板图片落盘与 NSPasteboard 兜底`
- `aeb5cd1` - `fix(terminal): 统一右键粘贴入口为外层自定义菜单，修复 TUI 横线问题`

当前 `pasteToTerminal()` 的策略是：

1. `readImage()` 直接取剪贴板像素
2. Tauri 落盘成 temp PNG
3. 把 temp PNG 路径写入 PTY
4. macOS / Windows 原生兜底继续返回路径
5. 最后才发 `Alt+V`

对应代码：

- `src/utils/terminalCache.ts:290`
- `src/utils/terminalCache.ts:291`
- `src/utils/terminalCache.ts:293`
- `src/utils/terminalCache.ts:324`

所以现在的结果是：

- 微信截图 / 系统截图 / 浏览器复制图片
- 在 Claude / Codex 里粘贴时
- 经常变成 `/tmp/mini-term-clipboard/clip-xxxx.png`
- 而不是 Claude/Codex 自己渲染的 `image #1`

## 目标

对 **AI TUI（Claude / Codex / Gemini 等支持图片粘贴的终端工具）**，调整为以下优先级：

1. **优先让 AI 工具自己吃图片粘贴事件**
   - 发送 `Alt+V`（`\x1bv`）
   - 让 Claude / Codex 自己从系统剪贴板读取图片
   - 这样输入框里会出现 `image #1` / `image1` 一类附件占位

2. **只有当 AI 原生图片粘贴不适用时，才退回 temp PNG 路径**
   - `readImage() -> save_clipboard_rgba_image -> path`
   - `read_clipboard_image_macos`
   - `read_clipboard_image`

3. **普通文本粘贴行为保持不变**

## 非目标

- 不改右键菜单结构
- 不删除现有 temp PNG 落盘能力
- 不重写全部剪贴板读取逻辑
- 不尝试做“真正判断 Alt+V 是否成功”的复杂握手协议

## 当前问题本质

### 现状

当前实现把“路径方案”放到了“AI 原生图片粘贴”前面。

也就是说：

- 只要 `readImage()` 成功
- 就直接生成 temp PNG 路径并写进 PTY
- 根本没有机会走 `Alt+V`

这正是你现在看到“变成路径”的直接原因。

### 为什么这不符合 AI TUI 预期

Claude / Codex 这类工具通常已经支持：

- 终端内图片粘贴
- 读取系统剪贴板中的图片
- 在输入框里生成附件占位（`image #1`）

所以在这些工具中，**最自然的行为不是输入文件路径，而是触发它们自己的图片附件流程**。

## 修复策略

核心思想：

- **先识别当前 PTY 是否处于 AI 会话 / AI pane**
- 如果是：
  - 优先发 `Alt+V`
- 如果不是：
  - 走现有 temp PNG 路径方案

即：

### AI pane

1. 图片剪贴板
2. 直接发 `\x1bv`
3. 若不适用再 fallback 到路径

### 非 AI pane

1. 图片剪贴板
2. 直接落盘成路径
3. 最后再 `Alt+V`

## 如何识别“当前是 AI pane”

当前仓库已经有 AI 状态/Provider 系统：

- `src/store.ts`
- `src-tauri/src/pty.rs`

可用信息：

- pane status
- AI provider
- 当前 pty 是否在 AI session 中

建议优先用前端现有 store 数据做判断，不新增复杂后端接口。

### 推荐方案

在前端新增 helper，例如：

```ts
function isAiPty(ptyId: number): boolean {
  const state = useAppStore.getState();
  for (const [, ps] of state.projectStates) {
    for (const tab of ps.tabs) {
      const found = findPaneByPty(tab.splitLayout, ptyId);
      if (found?.ptyId === ptyId) {
        return found.status === 'ai-generating'
          || found.status === 'ai-thinking'
          || found.status === 'ai-complete'
          || found.status === 'ai-awaiting-input';
      }
    }
  }
  return false;
}
```

如果已有公共 helper 可复用，优先复用，不要复制一套树遍历逻辑。

**目标不是 100% 精准识别 provider，而是优先区分：**

- 当前就是 Claude/Codex 这类 AI pane
- 还是普通 shell pane

## 实施顺序

---

### Task 1: 提炼“图片是否存在于剪贴板”判断

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 保留现有标准图片读取能力，但拆出布尔判断**

当前 `trySaveStandardClipboardImage()` 直接做“读取 + 落盘”。

为了支持“AI pane 先 Alt+V”，需要一个更轻量的判断：

```ts
async function clipboardHasImageData(): Promise<boolean> {
  try {
    const image = await readImage();
    await image.size();
    return true;
  } catch {
    return false;
  }
}
```

如果考虑兼容性，也可以继续保留现有 `readImage()` 失败再走平台兜底的判断方式，但目标是：

- 在 AI pane 里，不要一上来就保存成路径
- 先知道“有图”

---

### Task 2: 识别当前 PTY 是否属于 AI pane

**文件：**

- `src/utils/terminalCache.ts`
- 可能复用 `src/store.ts`
- 可能复用现有 split tree helper

- [ ] **Step 1: 增加 `isAiPty()` helper**

要求：

- 能从当前 store 里找到 `ptyId` 对应 pane
- 判断其状态是否是 AI 相关状态

推荐状态集合：

- `ai-generating`
- `ai-thinking`
- `ai-complete`
- `ai-awaiting-input`

如果当前类型定义中还有别的 AI status，也一并纳入。

- [ ] **Step 2: 不要为了这件事额外新开 Rust command**

这只是前端 routing 逻辑，优先用 store 现有信息完成。

---

### Task 3: 重排 `pasteToTerminal()` 优先级

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: AI pane 优先 `Alt+V`**

当前逻辑大致是：

```ts
const stdPath = await trySaveStandardClipboardImage();
if (stdPath) {
  await enqueuePtyWrite(ptyId, stdPath);
  return;
}
...
await enqueuePtyWrite(ptyId, '\x1bv');
```

改成：

```ts
const hasImage = await clipboardHasImageData();
const aiPane = isAiPty(ptyId);

if (hasImage && aiPane) {
  await enqueuePtyWrite(ptyId, '\x1bv');
  return;
}
```

然后再继续走非 AI / fallback 路径。

- [ ] **Step 2: 非 AI pane 保持路径优先**

对普通 shell / 非 AI pane：

- 继续优先 temp PNG 路径
- 这样不会破坏通用图片粘贴能力

- [ ] **Step 3: 最终建议结构**

推荐最终顺序：

```ts
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const hasImage = await clipboardHasImageData();
  const aiPane = isAiPty(ptyId);

  // 1. AI pane：优先让 Claude/Codex 自己接管图片粘贴
  if (hasImage && aiPane) {
    await enqueuePtyWrite(ptyId, '\x1bv');
    return;
  }

  // 2. 非 AI pane：继续走标准落盘路径
  const stdPath = await trySaveStandardClipboardImage();
  if (stdPath) {
    await enqueuePtyWrite(ptyId, stdPath);
    return;
  }

  // 3. macOS / Windows 原生兜底
  if (_isMacOS) {
    try {
      const path: string = await invoke('read_clipboard_image_macos');
      await enqueuePtyWrite(ptyId, path);
      return;
    } catch {}
  }

  if (_isWindows) {
    try {
      const path: string = await invoke('read_clipboard_image');
      await enqueuePtyWrite(ptyId, path);
      return;
    } catch {}
  }

  // 4. 文本
  const text = await readText().catch(() => null);
  if (text) {
    await enqueuePtyWrite(ptyId, text);
    return;
  }

  // 5. 最后保险
  if (hasImage) {
    await enqueuePtyWrite(ptyId, '\x1bv');
    return;
  }
}
```

### 关键约束

- AI pane 检测到图片时，不要先落盘成路径
- `Alt+V` 必须成为 AI pane 的**第一优先级**
- temp PNG 路径逻辑必须保留，作为非 AI / fallback 能力

---

### Task 4: 右键菜单与快捷键保持同一策略

**文件：**

- `src/components/TerminalInstance.tsx`
- `src/utils/terminalCache.ts`

- [ ] **Step 1: 右键菜单“粘贴”继续只调 `pasteToTerminal()`**

不要写两套图片粘贴策略。

目标：

- `Ctrl+Shift+V`
- 右键菜单 -> 粘贴

二者完全共享同一个 `pasteToTerminal()`。

- [ ] **Step 2: 聚焦处理保留**

继续在菜单点击后 focus terminal，避免 UI 交互掉焦点。

---

### Task 5: 如有必要，支持 macOS `Command+V`

**文件：**

- `src/utils/terminalCache.ts`

现在代码只拦了：

```ts
if (e.ctrlKey && e.shiftKey && e.code === 'KeyV') { ... }
```

如果产品想要更接近 mac 用户直觉，可额外支持：

- `metaKey && code === 'KeyV'`

但这一步是可选项。

**注意：**

- 如果加 `Command+V`
- 要确保不会和系统默认粘贴 / xterm 自身处理打架
- 必须 `preventDefault()`

如果不确定，就先不加，只先修“AI pane 优先 Alt+V”。

---

### Task 6: 验证

- [ ] **Step 1: 前端构建**

```bash
npm run build
```

- [ ] **Step 2: Rust 编译**

```bash
cd src-tauri && cargo check
```

- [ ] **Step 3: Claude / Codex 图片粘贴**

在 AI 对话 pane 中：

- 复制微信截图
- 测试 `Ctrl+Shift+V`
- 测试右键菜单 -> 粘贴

预期：

- 优先出现 `image #1` / `image1`
- 不再优先出现 temp PNG 路径

- [ ] **Step 4: 普通 shell pane 图片粘贴**

在非 AI pane：

- 复制截图
- 测试粘贴

预期：

- 仍然可以走 temp PNG 路径方案
- 不会把所有图片粘贴都硬变成 `Alt+V`

- [ ] **Step 5: 文本粘贴不回退**

文本剪贴板：

- `Ctrl+Shift+V`
- 右键菜单 -> 粘贴

预期：

- 继续正常输入文本

## 风险点

### 1. 不能把所有图片粘贴都变成 `Alt+V`

否则普通 shell / 非 AI pane 会失去可用性。

必须按“AI pane / 非 AI pane”分流。

### 2. AI pane 判断不要只靠“当前激活项目”

判断维度要落到具体 `ptyId` / pane，而不是项目级。

### 3. `Alt+V` 不是百分百有回执

这次不要做复杂的“是否成功上传图片”协议。

先实现：

- AI pane 优先 `Alt+V`
- 失败场景保留 fallback

如果后续还需要更智能的确认，再做第二轮。

## 完成定义

满足以下条件才算完成：

- Claude / Codex 对话 pane 粘贴微信截图时，优先出现 `image #1` / `image1`
- 不再优先出现 temp PNG 路径
- 右键菜单与快捷键行为一致
- 普通 shell pane 仍保留路径 fallback
- 文本粘贴不受影响
- `npm run build` 通过
- `cargo check` 通过

## 建议提交信息

```bash
fix(clipboard): AI pane 优先使用原生图片粘贴而非路径回填
```
