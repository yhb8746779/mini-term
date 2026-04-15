# 跨平台终端右键 / 图片粘贴最终收敛方案

> 这份文档作为当前这一轮终端剪贴板改造的最终收口稿，给 Claude Code 直接照着改。它覆盖并收敛同目录下几份更早的图片粘贴 follow-up 文档，重点解决 3 个问题：
> 
> 1. mac / Windows 右键行为不一致
> 2. AI pane 的图片粘贴仍然过度依赖 provider 分流
> 3. “原始剪贴板图片” 与 “复制的图片文件 / 富对象” 还没有统一建模

## 一句话结论

终端右键不要再按“菜单动作”思维拆了，而要按“语义动作”收敛：

- **有选中** -> 直接复制
- **无选中** -> 直接粘贴
- **粘贴时再按剪贴板内容类型 + pane 类型决定真正怎么处理**

也就是说，右键本身只是入口，真正的分流逻辑应该全部收敛进 `pasteToTerminal()`。

---

## 1. 当前代码现状

### 1.1 `src/components/TerminalInstance.tsx`

当前右键逻辑是：

- Windows：右键直接执行
  - 有选中 -> 复制
  - 无选中 -> `pasteToTerminal()`
- macOS / Linux：仍然弹自定义菜单

这会带来两个问题：

1. mac 端用户比 Windows 多一次点击，体验不统一
2. 右键菜单路径天然更容易引入焦点丢失、选区丢失、菜单时序问题

### 1.2 `src/utils/terminalCache.ts`

当前 `pasteToTerminal()` 的核心逻辑仍然是：

1. `clipboardHasImageData()`
2. `getAiProviderForPty(ptyId)`
3. 如果 `hasImage && provider` -> 发送 provider-specific 图片快捷键
4. 否则走 temp PNG / 原生兜底 / 文本

当前的主要问题：

- AI pane 仍然是 **provider + platform** 双重分流
- AI 分支只在 `readImage()` 能读到原始像素时才触发
- 复制图片文件、文件对象、富剪贴板对象这类场景，容易被错误归到普通文本 / 路径分支

### 1.3 `src-tauri/src/clipboard.rs`

Rust 侧当前已经具备：

- 通用 RGBA -> temp PNG 落盘
- macOS `NSPasteboard` 图片读取兜底
- Windows `CF_DIB / CF_BITMAP` 图片读取兜底

这部分能力本身没有问题，问题主要不在解码，而在 **前端路由策略**。

---

## 2. 已确认的真实行为（以你的实机测试为准）

下面这些结论，已经足够作为本轮实现依据。

### 2.1 原始剪贴板图片

典型场景：微信截图、截图工具确认后，图片内容直接进系统剪贴板。

已确认：

- **macOS / AI CLI**：图片粘贴走 `Ctrl+V`
- **Windows / AI CLI**：图片粘贴走 `Alt+V`
- **Linux / AI CLI**：当前没有稳定实测，第一轮先按 `Ctrl+V` 兜底

### 2.2 复制的图片文件

典型场景：在 Finder / Explorer 里复制一张图片文件，然后到 AI CLI 输入区里粘贴。

你已确认：

- 在原生 **Windows Terminal** 上，右键可直接以“图片”的样子进入 AI CLI
- 在原生 **macOS Terminal** 上，右键也可直接以“图片”的样子进入 AI CLI

这个现象说明：

- AI pane 不能只看“是否存在原始像素图片”
- “复制图片文件”这类 **富剪贴板对象**，在 AI pane 里也应该优先走“让 CLI 自己处理剪贴板”的路线
- 不能过早把它强行转换成 temp PNG 路径或纯文本路径

### 2.3 普通 shell / 非 AI pane

这里的目标不变：

- 文本 -> 直接粘贴文本
- 图片 -> temp PNG 路径
- 平台原生图片读取失败 -> 最后再做保底降级

也就是说，**AI pane** 和 **普通 shell pane** 的策略本来就不该完全一样。

---

## 3. 这轮要达成的统一行为

## 3.1 右键交互统一规则

### macOS + Windows

统一成：

- **有选中** -> 右键直接复制
- **无选中** -> 右键直接粘贴

不再先弹菜单。

原因：

1. 更接近原生终端
2. 比菜单少一步
3. 可以避开一整类焦点 / 选区 / 菜单关闭时序 bug

### Linux

第一轮先保守：

- 可以继续保留菜单
- 等真实测试后再决定是否也切到“右键直接执行”

这样改动最小，风险也最低。

## 3.2 粘贴分流统一规则

右键无选中时，统一只做一件事：

- 调 `pasteToTerminal(ptyId)`

真正的判断全部下沉到 `pasteToTerminal()`：

### 先判断 pane 类型

- AI pane
- 非 AI pane

### 再判断剪贴板内容类型

建议统一成 4 类：

- `plain-text`：明确纯文本
- `raw-image`：剪贴板里就是原始图片像素
- `rich-object`：文件对象 / 图片文件 / 非纯文本富对象 / 无法安全归类为纯文本的对象
- `empty-or-unknown`：什么都没读出来

---

## 4. 最终行为矩阵

## 4.1 AI pane

### 有选中

- 右键 -> 直接复制

### 无选中 + `plain-text`

- 直接写文本到 PTY

### 无选中 + `raw-image`

- **macOS** -> 发送 `Ctrl+V`
- **Windows** -> 发送 `Alt+V`
- **Linux** -> 第一轮先发送 `Ctrl+V`

### 无选中 + `rich-object`

这里要特别注意：

- 这类对象不要直接落盘成 temp PNG 路径
- 也不要直接退化成普通文本路径
- 应该优先走“让 AI CLI 自己从系统剪贴板读取”的路线

第一轮推荐策略：

- `rich-object` 在 AI pane 中，**和 `raw-image` 一样先走平台级 native paste shortcut**
  - macOS -> `Ctrl+V`
  - Windows -> `Alt+V`
  - Linux -> `Ctrl+V`

这样做的原因：

- 这比“先变 temp 文件路径”更接近你在原生终端测到的行为
- 对 AI CLI 来说，富剪贴板对象往往应该交给 CLI 自己判断，而不是 Mini-Term 先做路径化

这里要明确：

- 这部分是**基于你原生终端实测后的工程推断**
- 不是浏览器层面已经 100% 证明的事实
- 但它是当前最合理、最接近原生行为的实现方向

### 无选中 + `empty-or-unknown`

- AI pane 里，优先按 `rich-object` 处理
- 也就是仍然先发平台 native paste shortcut
- 不要默认立刻退成 temp 路径

原因是：

- 很多系统 / WebView / Clipboard API 组合下，富对象不一定能被前端精确识别
- 但“不是明确纯文本”时，优先交给 AI CLI 自己处理，成功率往往更高

## 4.2 非 AI pane

### 有选中

- 右键 -> 直接复制

### 无选中 + `plain-text`

- 直接写文本到 PTY

### 无选中 + `raw-image`

继续当前策略：

1. `readImage()` 落盘 temp PNG
2. macOS `read_clipboard_image_macos`
3. Windows `read_clipboard_image`
4. 都失败再做最后兜底

### 无选中 + `rich-object`

非 AI pane 不追求“图片附件”语义，允许保守一点：

- 如果能读到文本，就贴文本
- 如果能落成图片路径，就贴路径
- 如果都不行，再兜底

也就是说，`rich-object` 的“优先交给 CLI 自己处理”只属于 AI pane。

---

## 5. 为什么不要再继续按 provider 分流

当前代码里 `getProviderImagePasteShortcut(provider)` 的思路已经不适合继续扩展了。

原因：

1. **问题本质是平台差异，不是 provider 差异**
   - 你最新实测已经表明：真正稳定的规律主要是 mac / Windows 的差异

2. **provider 分流会把逻辑越写越碎**
   - `claude`、`codex`、`gemini` 三家后面一旦再出现小差异，代码会越来越难维护

3. **右键不是键盘快捷键的简单镜像**
   - 右键是一个“语义动作”，不应该把它理解成“模拟某个 provider 的某组按键”
   - 正确做法是：右键先判定“当前应该执行什么语义”，然后内部再决定发文本、发平台快捷键，还是走 temp 路径

因此，本轮应该改成：

- provider 只用来判断“当前是不是 AI pane”
- 一旦确认是 AI pane，图片 / 富对象粘贴就按平台走，不再按 provider 细分

---

## 6. 代码改造建议

## Task 1: `TerminalInstance.tsx` 统一 mac / Windows 右键行为

**文件：** `src/components/TerminalInstance.tsx`

### 目标

把当前：

- Windows 直接执行
- mac / Linux 弹菜单

改成：

- Windows / macOS 直接执行
- Linux 暂时保留菜单

### 推荐逻辑

```ts
const handleContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
  e.preventDefault();
  const selectedText = getAnySelectedText(ptyId);

  if (_isWindows || _isMacOS) {
    if (selectedText) {
      void copyTextToClipboard(selectedText);
      getCachedTerminal(ptyId)?.term.clearSelection();
    } else {
      void pasteToTerminal(ptyId).finally(() => {
        getCachedTerminal(ptyId)?.term.focus();
      });
    }
    return;
  }

  // Linux 先保留菜单
  showContextMenu(...)
};
```

### 说明

- mac 是否清除选区可以后调，但第一轮为了统一行为，可以和 Windows 一样先清掉
- 重点不是“清不清选区”，而是“不要再弹菜单”

---

## Task 2: `terminalCache.ts` 把 AI 粘贴从 provider 分流改成平台分流

**文件：** `src/utils/terminalCache.ts`

### 当前需要删除或重构的部分

- `getProviderImagePasteShortcut(provider)`
- `sendProviderImagePasteShortcut(ptyId, provider)`

### 建议替换为

```ts
type NativePasteShortcut = 'ctrl-v' | 'alt-v';

function getPlatformAiPasteShortcut(): NativePasteShortcut {
  if (_isMacOS) return 'ctrl-v';
  if (_isWindows) return 'alt-v';
  return 'ctrl-v';
}

async function sendPlatformAiPasteShortcut(ptyId: number): Promise<void> {
  const shortcut = getPlatformAiPasteShortcut();
  if (shortcut === 'alt-v') {
    await enqueuePtyWrite(ptyId, '\x1bv');
    return;
  }
  await enqueuePtyWrite(ptyId, '\x16');
}
```

### 说明

- `getAiProviderForPty()` 可以保留
- 但建议后续把它改名成更准确的 `isAiPaneForPty()` 或 `getAiPaneForPty()`
- 这轮不要再让 provider 参与图片 shortcut 决策

---

## Task 3: 增加统一的剪贴板分类器

**文件：** `src/utils/terminalCache.ts`

当前最大问题不是“不会读图片”，而是“不会正确判断现在该走哪条路”。

建议加一个统一 helper，例如：

```ts
type ClipboardPayloadKind =
  | 'plain-text'
  | 'raw-image'
  | 'rich-object'
  | 'empty-or-unknown';
```

### 推荐判定原则

#### `plain-text`

满足下面条件时才算：

- 能稳定读到文本
- 且没有明显图片 / 文件 / 富对象迹象

#### `raw-image`

满足任一：

- `readImage()` 成功
- `navigator.clipboard.read()` 中含有 `image/*`

#### `rich-object`

满足任一：

- `navigator.clipboard.read()` 中含文件 / URI / 非纯文本对象
- 不是明确纯文本，但又不是简单空剪贴板
- 对 AI pane 来说，这类对象应视为“优先交给 CLI 自己处理”

#### `empty-or-unknown`

- API 全失败
- 看不出是文本，也看不出是图片，但也不能证明里面没有富对象

### 实现建议

优先尝试：

```ts
navigator.clipboard.read()
```

去看 `ClipboardItem.types`，例如：

- `image/png`
- `image/tiff`
- `public.file-url`
- `text/uri-list`
- 其他 file / image / binary 类型

如果这一层拿不到，再 fallback：

- `readImage()`
- `readText()`

### 很关键的一点

**不要把“能读到一点 text/plain”就直接判成纯文本。**

有些富对象剪贴板可能会同时带一点文本表示，但真实意图并不是普通文本粘贴。

所以更稳的判断顺序应该是：

1. 先看有没有图片 / 文件 / 富对象迹象
2. 没有这些迹象时，才把它视作纯文本

---

## Task 4: 重写 `pasteToTerminal()` 的分流顺序

**文件：** `src/utils/terminalCache.ts`

### 推荐顺序

```ts
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const isAiPane = !!getAiProviderForPty(ptyId);
  const clipboard = await detectClipboardPayload();

  if (isAiPane) {
    if (clipboard.kind === 'plain-text' && clipboard.text) {
      await enqueuePtyWrite(ptyId, clipboard.text);
      return;
    }

    // raw-image / rich-object / empty-or-unknown
    await sendPlatformAiPasteShortcut(ptyId);
    return;
  }

  if (clipboard.kind === 'plain-text' && clipboard.text) {
    await enqueuePtyWrite(ptyId, clipboard.text);
    return;
  }

  if (clipboard.kind === 'raw-image') {
    const stdPath = await trySaveStandardClipboardImage();
    if (stdPath) {
      await enqueuePtyWrite(ptyId, stdPath);
      return;
    }

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
  }

  if (clipboard.text) {
    await enqueuePtyWrite(ptyId, clipboard.text);
    return;
  }

  // 最后保险
  if (isAiPane) {
    await sendPlatformAiPasteShortcut(ptyId);
    return;
  }

  await enqueuePtyWrite(ptyId, '\x1bv');
}
```

### 关键变化

和现在相比，真正的变化不是“多一个 API”，而是：

- AI pane 不再只看 `hasImage`
- AI pane 只要不是明确纯文本，就优先让 CLI 自己处理剪贴板
- 非 AI pane 继续保留 temp PNG / 原生图片兜底能力

---

## 7. 关于“Command+V / Control+V / Alt+V”的正确理解

这一点必须在文档里讲清楚，不然后面很容易再绕回 provider 分流。

### 7.1 键盘快捷键和右键粘贴不是一回事

用户前面担心过：

- mac 上 Claude Code 是不是 `Command+V`
- Codex 是不是 `Control+V`
- Windows 不同 CLI 会不会也不一样

这里需要收敛成一个更稳定的原则：

- **键盘路径**：保留各自现有快捷键，不要在这轮大动
- **右键路径**：不要等价成“模拟某个键盘组合”
- 右键应该是“语义粘贴”，内部再决定真正执行什么

### 7.2 对 Mini-Term 来说，右键只需要关心 3 件事

- 当前有没有选区
- 当前是不是 AI pane
- 当前剪贴板是不是明确纯文本

除此之外的复杂度，都应该藏进 `pasteToTerminal()` 里。

---

## 8. 不建议做的事

本轮不要做下面这些：

1. 不要继续细化到 `claude/codex/gemini` 三套右键图片逻辑
2. 不要把 AI pane 的富对象一上来就转 temp 文件路径
3. 不要为了右键去改 xterm 光标样式
4. 不要把 Linux 也一起强行改成和 mac / Windows 完全同一套，先保守
5. 不要同时重写 Rust 侧图片解码；本轮核心是前端路由，不是底层解码

---

## 9. 建议验证清单

## 9.1 macOS

### AI pane

- [ ] 微信截图确认后，右键无选中 -> 是否按图片附件进入
- [ ] Finder 复制图片文件后，右键无选中 -> 是否按图片附件进入
- [ ] 纯文本 -> 是否直接贴文本
- [ ] 有选中文字 -> 右键是否直接复制

### 非 AI pane

- [ ] 纯文本 -> 正常粘贴
- [ ] 剪贴板图片 -> 是否仍然走 temp PNG 路径

## 9.2 Windows

### AI pane

- [ ] 剪贴板截图 -> 是否按图片附件进入
- [ ] Explorer 复制图片文件 -> 是否按图片附件进入
- [ ] 纯文本 -> 是否直接贴文本
- [ ] 有选中文字 -> 右键是否直接复制并清选区

### 非 AI pane

- [ ] 图片 -> 是否仍然落 temp PNG 路径
- [ ] 文本 -> 是否正常粘贴

## 9.3 Linux

### AI pane

- [ ] 原始剪贴板图片 -> `Ctrl+V` 是否可用
- [ ] 文件对象剪贴板 -> 是否需要额外分流

### UI

- [ ] 先允许右键菜单继续存在

---

## 10. 完成定义

满足下面这些，才算这一轮真正收敛完成：

- mac / Windows 终端右键统一成“有选中复制、无选中粘贴”
- AI pane 的粘贴逻辑不再主要按 provider 分流，而是按平台分流
- AI pane 不再只依赖 `clipboardHasImageData()` 才决定走 native paste
- 复制图片文件 / 富对象剪贴板不再被过早退化成 temp 路径
- 非 AI pane 继续保留 temp PNG / 原生图片兜底能力
- 焦点不丢，右键后仍可继续输入
- `npm run build` 通过
- 有条件的话，再补一轮 Windows / Linux 实机验证

---

## 11. 推荐提交信息

```bash
refactor(clipboard): unify right-click and ai paste routing by platform
```
