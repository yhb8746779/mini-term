# Provider 级图片粘贴路由 Follow-up 计划

> **For Claude Code / agentic workers:** 本文档处理的是“不同 AI CLI 的图片粘贴触发键并不完全相同”这个问题。当前实现只区分“AI pane / 非 AI pane”，但没有区分 `claude` / `codex` / `gemini`。这会导致某些 provider 在 macOS / Windows 上图片粘贴表现不一致。  
> 同时，本文档也给出终端右键菜单是否保留、如何精简为“选中即复制 / 未选中即粘贴”的建议。

## 背景

当前最新相关提交：

- `00a0c0b` - `fix(clipboard): AI pane 优先使用原生图片粘贴而非路径回填`
- `aeb5cd1` - `fix(terminal): 统一右键粘贴入口为外层自定义菜单，修复 TUI 横线问题`
- `57cb3fd` - `fix(clipboard): 补齐 macOS 剪贴板图片落盘与 NSPasteboard 兜底`

当前代码的核心问题在于：

- `src/utils/terminalCache.ts` 虽然已经能识别“当前是 AI pane”
- 但只用了 `isAiPty(ptyId)`
- 对所有 AI pane 一律发送同一个按键序列：
  - `\x1bv`

对应代码：

- `src/utils/terminalCache.ts:297`
- `src/utils/terminalCache.ts:325`
- `src/utils/terminalCache.ts:329`

也就是说当前逻辑是：

- Claude pane -> `Alt+V`
- Codex pane -> `Alt+V`
- Gemini pane -> `Alt+V`

这对 `codex` 可能有效，但对 `claude` 很可能不对。

## 用户反馈要解决的实际问题

当前真实用户观察：

- Codex 里右键复制/粘贴已经基本可用
- 但 Claude Code 里图片粘贴仍不对
- 用户怀疑：
  - macOS 上 Claude Code 的图片粘贴更接近 `Command+V`
  - Codex 则更像 `Control+V`

从当前代码上看，这种怀疑是**完全成立的方向**，因为现有实现根本没有 provider 分流。

## 外部资料核对结果

我查了官方/一手资料与官方仓库：

### 1. Claude Code

Anthropic 官方文档显示：

- 交互模式文档写的是：
  - `Ctrl+V`
  - 或 `Cmd+V (iTerm2)`
  - 或 `Alt+V (Windows)`
  - 用于“Paste image from clipboard”
- 另一页常见工作流明确写：
  - 粘贴图片请用 `ctrl+v`
  - **Do not use `cmd+v`**

结论：

- Claude Code 的图片粘贴确实是 provider-aware / terminal-aware 的
- 在 macOS 上不能简单假设“永远都是 `Alt+V`”
- `Cmd+V` 只在某些终端环境（文档明确点名 iTerm2）才可能成立
- 最保守可靠的默认值仍应优先尝试 `Ctrl+V`
- Windows 明确是 `Alt+V`

来源：

- Anthropic Claude Code docs / interactive mode  
  <https://code.claude.com/docs/en/interactive-mode>
- Anthropic Claude Code docs / common workflows  
  <https://code.claude.com/docs/en/common-workflows>

### 2. Codex

从 OpenAI 官方仓库公开 issue / maintainer 评论里可以确认：

- Codex 的图片粘贴主认知是 `Ctrl+V`
- macOS 用户公开复现里也明确提到：
  - `Cmd+V` 对图片常常不工作
  - `Ctrl+V` 可以附图
- Windows 上存在终端差异：
  - 官方仓库 discussion / issue 片段里，OpenAI 维护者提到 Windows 终端常会吞掉普通 paste event
  - `Ctrl+Shift+V` 在某些 Windows 终端里更可靠

结论：

- Codex 在 macOS 上更应该优先尝试 `Ctrl+V`
- Windows 上要兼容：
  - `Ctrl+V`
  - `Ctrl+Shift+V`

来源：

- OpenAI Codex repo issue: macOS image paste behavior  
  <https://github.com/openai/codex/issues/3397>
- OpenAI Codex repo issue: Windows image paste behavior / maintainer comment snippet  
  <https://github.com/openai/codex/issues/2597>
- OpenAI Codex repo issue: image placeholder on paste  
  <https://github.com/openai/codex/issues/4818>

### 3. Gemini CLI

Gemini 官方快捷键文档写得最直接：

- `Ctrl+V`
- “Paste clipboard content. If the clipboard contains an image, it will be saved and a reference to it will be inserted in the prompt.”

结论：

- Gemini CLI 当前最明确的图片粘贴键是 `Ctrl+V`
- 没看到官方文档为 Gemini 单独列出 macOS `Cmd+V` 或 Windows `Alt+V`

来源：

- Gemini CLI keyboard shortcuts  
  <https://google-gemini.github.io/gemini-cli/docs/cli/keyboard-shortcuts.html>

## 最终结论

### 不能再只用一个统一的 `Alt+V`

当前代码：

```ts
if (hasImage && isAiPty(ptyId)) {
  await enqueuePtyWrite(ptyId, '\x1bv');
  return;
}
```

这对 provider 差异完全不敏感，必须改。

### 建议的 provider / platform 路由

#### macOS

- `claude`
  - 优先：`Ctrl+V`
  - 可选 fallback：`Cmd+V`（仅在特定终端环境下）
- `codex`
  - 优先：`Ctrl+V`
- `gemini`
  - 优先：`Ctrl+V`

#### Windows

- `claude`
  - 优先：`Alt+V`（Anthropic 官方文档明确）
- `codex`
  - 建议尝试：
    1. `Ctrl+V`
    2. `Ctrl+Shift+V`
- `gemini`
  - 优先：`Ctrl+V`
  - 如本地验证发现 Windows 终端吞 `Ctrl+V`，再考虑补 `Ctrl+Shift+V`

## 当前代码中已经有的可利用信息

仓库已经有 provider 识别能力：

- `src/types.ts:68`
- `src/types.ts:103`
- `src/store.ts:480`

`PaneState` 已经带：

```ts
aiProvider?: AiProvider;
```

而当前 `pasteToTerminal()` 只判断：

```ts
isAiPty(ptyId)
```

所以需要新增一个更强的 helper，例如：

```ts
function getAiProviderForPty(ptyId: number): AiProvider | null
```

## 实施顺序

---

### Task 1: 用 provider 替换掉单纯的 `isAiPty()`

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 新增 `getAiProviderForPty()`**

目标：

- 从当前 store 里遍历 projectStates / tabs / splitLayout
- 找到给定 `ptyId` 对应的 pane
- 返回：
  - `'claude'`
  - `'codex'`
  - `'gemini'`
  - `null`

伪代码：

```ts
function getAiProviderForPty(ptyId: number): AiProvider | null {
  const { projectStates } = useAppStore.getState();
  for (const ps of projectStates.values()) {
    for (const tab of ps.tabs) {
      const pane = findPaneByPty(tab.splitLayout, ptyId);
      if (pane) {
        return pane.aiProvider ?? null;
      }
    }
  }
  return null;
}
```

- [ ] **Step 2: `isAiPty()` 可以保留，但不再作为图片粘贴唯一依据**

保留也可以，供其他场景复用。

但图片粘贴决策必须升级为：

- `provider`
- `platform`
- `clipboard has image`

三维判断。

---

### Task 2: 抽象“发送 provider 专属图片粘贴按键”

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 新增平台判断**

当前已有：

```ts
const _isMacOS = /Mac OS X|Macintosh/.test(navigator.userAgent);
const _isWindows = /Windows/.test(navigator.userAgent);
```

继续复用即可。

- [ ] **Step 2: 新增 helper**

推荐新增：

```ts
async function sendProviderImagePasteShortcut(
  ptyId: number,
  provider: AiProvider
): Promise<boolean>
```

语义：

- 返回 `true`：说明已经发送了某个 provider 专属图片粘贴序列
- 返回 `false`：说明当前 provider/platform 没有专属策略，应继续走路径 fallback

### 推荐路由实现

```ts
async function sendProviderImagePasteShortcut(
  ptyId: number,
  provider: AiProvider
): Promise<boolean> {
  if (_isMacOS) {
    if (provider === 'claude') {
      // Claude docs on macOS: prefer Ctrl+V; Cmd+V only in some terminals (e.g. iTerm2)
      await enqueuePtyWrite(ptyId, '\x16'); // Ctrl+V
      return true;
    }
    if (provider === 'codex' || provider === 'gemini') {
      await enqueuePtyWrite(ptyId, '\x16'); // Ctrl+V
      return true;
    }
    return false;
  }

  if (_isWindows) {
    if (provider === 'claude') {
      await enqueuePtyWrite(ptyId, '\x1bv'); // Alt+V
      return true;
    }
    if (provider === 'codex') {
      // 第一版可先用 Ctrl+V；必要时扩展 Ctrl+Shift+V 特殊路径
      await enqueuePtyWrite(ptyId, '\x16'); // Ctrl+V
      return true;
    }
    if (provider === 'gemini') {
      await enqueuePtyWrite(ptyId, '\x16'); // Ctrl+V
      return true;
    }
  }

  // Linux / 其他：先用 Ctrl+V 作为通用 AI CLI 路径
  await enqueuePtyWrite(ptyId, '\x16');
  return true;
}
```

**注意：**

- `Ctrl+V` 对应控制字符 `\x16`
- 不是字面量 `"Ctrl+V"`
- 当前实现用 `\x1bv` 发送的是 `Alt+V`

---

### Task 3: 重写 `pasteToTerminal()` 的 AI 分支

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 从“AI pane”升级为“AI provider”**

当前：

```ts
if (hasImage && isAiPty(ptyId)) {
  await enqueuePtyWrite(ptyId, '\x1bv');
  return;
}
```

改成：

```ts
const provider = getAiProviderForPty(ptyId);
if (hasImage && provider) {
  const handled = await sendProviderImagePasteShortcut(ptyId, provider);
  if (handled) return;
}
```

- [ ] **Step 2: provider 专属快捷键失败后再走现有 path fallback**

要求：

- 不删除：
  - `trySaveStandardClipboardImage()`
  - `read_clipboard_image_macos`
  - `read_clipboard_image`
- provider 专属路径只是更前面的优先级

最终顺序应是：

1. 检测是否有图片
2. 若当前 pane 有 AI provider：
   - 尝试 provider/platform 专属粘贴快捷键
3. 若未处理：
   - 走 temp PNG 路径
4. 若仍失败：
   - 文本
5. 最后保险 fallback

---

### Task 4: Windows provider 差异要预留机制

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 为 Windows provider 路由写成表驱动/策略函数**

不要把所有条件硬编码进 `pasteToTerminal()`。

建议写成：

```ts
type ImagePasteStrategy =
  | 'ctrl-v'
  | 'alt-v'
  | 'ctrl-shift-v'
  | 'path-fallback';
```

然后：

```ts
function getImagePasteStrategy(provider: AiProvider, platform: 'mac' | 'windows' | 'linux'): ImagePasteStrategy[]
```

例如：

```ts
claude + mac     => ['ctrl-v', 'path-fallback']
claude + windows => ['alt-v', 'path-fallback']
codex + mac      => ['ctrl-v', 'path-fallback']
codex + windows  => ['ctrl-v', 'ctrl-shift-v', 'path-fallback']
gemini + mac     => ['ctrl-v', 'path-fallback']
gemini + windows => ['ctrl-v', 'path-fallback']
```

这样后续如果某个 provider 变更快捷键，改表即可。

---

### Task 5: 右键菜单是否保留 —— 结论与建议

## 结论

**建议保留右键菜单，并继续同时显示“复制 + 粘贴”两个选项。**

但需要微调按钮状态逻辑：

- **有选中**
  - `复制`：可点击
  - `粘贴`：也可点击
- **无选中**
  - `复制`：不可点击
  - `粘贴`：可点击

## 理由

### 1. 菜单仍然有必要

因为图片粘贴现在已经变成了：

- provider-aware
- platform-aware
- terminal-aware

这已经不是一个简单的“系统默认 paste”动作了。

保留菜单的好处：

- 避免右键事件本身直接干扰 TUI
- 用户明确点“粘贴”后再执行 provider 专属逻辑
- 将来若要补“粘贴图片 / 粘贴文本 / 粘贴文件路径”子策略，也有扩展空间

### 2. 菜单没必要收缩成“只显示一个选项”

原先“有选中就只显示复制、无选中就只显示粘贴”的建议，虽然更简洁，但不够贴近真实桌面交互。

因为在输入框 / 对话区域里，真实用户可能会有这两种连续动作：

- 先选中一段内容，准备复制
- 但仍然也可能希望直接点“粘贴”去覆盖或继续输入

所以更合理的方案是：

- **有选中时：复制能点，粘贴也能点**
- **无选中时：复制禁用，粘贴可点**

### 3. 推荐最终 UX

- **始终显示**
  - `复制`
  - `粘贴`
- **动态状态**
  - 有选中：`复制` 启用
  - 无选中：`复制` 禁用
  - `粘贴` 始终启用

这既保留了菜单的安全性，也更符合桌面应用习惯。

---

### Task 6: 调整 `TerminalInstance.tsx` 的菜单项

**文件：**

- `src/components/TerminalInstance.tsx`

- [ ] **Step 1: 保持双菜单项，不改显示结构**

当前固定两项的方向是对的：

```tsx
showContextMenu(e.clientX, e.clientY, [
  { label: '复制', disabled: !hasSelection, ... },
  { label: '粘贴', ... },
]);
```

这里不要改成“只显示一个按钮”的菜单。

- [ ] **Step 2: 复制仅在有选中时启用**

保留：

```tsx
disabled: !hasSelection
```

这能满足：

- 对话页/输入区有选中时，`复制` 可以点
- 没选中时，`复制` 不可点

- [ ] **Step 3: 粘贴始终可点**

要求：

- 即使当前存在选中，`粘贴` 也不要禁用
- 这样用户在输入区选中内容后，仍可直接点粘贴

推荐保持：

```tsx
{
  label: '粘贴',
  onClick: () => {
    void pasteToTerminal(ptyId).finally(() => {
      getCachedTerminal(ptyId)?.term.focus();
    });
  },
}
```

## 这样做的理由

- 符合桌面应用常见习惯
- 兼顾“选中后复制”和“选中后直接粘贴”的两类真实操作
- 不影响 provider-aware 图片粘贴主逻辑

---

### Task 7: 验证

- [ ] **Step 1: Claude on macOS**

验证：

- 微信截图
- 在 Claude Code pane 中：
  - 快捷键粘贴
  - 右键菜单 -> 粘贴

预期：

- 不再错误走统一 `Alt+V`
- 优先出现 `[Image #N]` / `image #1`

- [ ] **Step 2: Codex on macOS**

验证：

- 图片粘贴继续可用
- 不退化成路径优先

- [ ] **Step 3: Gemini on macOS**

验证：

- `Ctrl+V` 图片粘贴仍可工作

- [ ] **Step 4: Windows provider 差异验证**

分别验证：

- Claude on Windows
- Codex on Windows
- Gemini on Windows

记录哪一种最稳定：

- `Alt+V`
- `Ctrl+V`
- `Ctrl+Shift+V`

如果本地验证与当前策略表不一致，再更新策略表。

- [ ] **Step 5: 右键菜单验证**

验证：

- 菜单始终显示：
  - `复制`
  - `粘贴`
- 有选中：
  - `复制` 可点
  - `粘贴` 也可点
- 无选中：
  - `复制` 不可点
  - `粘贴` 可点

## 完成定义

满足以下条件才算完成：

- 图片粘贴不再只按“AI / 非 AI”分流，而是按 provider + platform 分流
- Claude / Codex / Gemini 在 macOS 上使用正确的图片粘贴触发键
- Windows 上预留并实现 provider-specific 处理机制
- 右键菜单继续保留双选项：
  - `复制`
  - `粘贴`
- 且状态满足：
  - 有选中时 `复制` 可点
  - `粘贴` 始终可点
- `npm run build` 通过
- `cargo check` 通过

## 建议提交信息

```bash
fix(clipboard): 按 provider 和平台分流图片粘贴快捷键
```
