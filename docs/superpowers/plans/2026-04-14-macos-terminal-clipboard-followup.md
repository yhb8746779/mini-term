# macOS 终端乱码 + 右键图片粘贴 Follow-up 计划

> **For Claude Code / agentic workers:** 本文档合并处理两个 macOS 回归/缺口：
>
> 1. **终端乱码问题**：macOS 上中文 / emoji / CJK 输出再次出现乱码或编码异常
> 2. **右键图片粘贴问题**：微信截图等图片在 macOS 上用终端区域右键粘贴时，行为与快捷键不一致，表现为卡顿、TUI 出现横线/异常 UI，而不是像 `Ctrl+Shift+V` 那样插入 `image1`
>
> 不要重做整个终端或剪贴板系统，只修这两个问题对应的缺口。

## 背景

当前分支最近与这两个问题相关的提交：

- `194c2e3` - `feat(clipboard): 图片粘贴支持 + macOS Ctrl 键修复`
- `57cb3fd` - `fix(clipboard): 补齐 macOS 剪贴板图片落盘与 NSPasteboard 兜底`
- `6d55759` - `fix: 修复 macOS CJK 中文乱码（Unicode11Addon + CJK 备用字体）`
- `96be9d8` - `fix: 修复 code review 反馈的问题`（这里把 PTY locale 注入改成了只设 `LC_CTYPE=UTF-8`）

当前代码现状：

- `src/utils/terminalCache.ts`
  - 已有 `Unicode11Addon`
  - 已有 CJK fallback fonts
  - 已有图片粘贴主路径：`readImage() -> save_clipboard_rgba_image`
  - **但内层 xterm wrapper 还在直接拦截 `contextmenu` 并立刻执行 paste**
- `src/components/TerminalInstance.tsx`
  - 已有外层自定义右键菜单 `showContextMenu(...)`
  - **但真实终端区域右键通常先被内层 wrapper 截走**
- `src-tauri/src/pty.rs`
  - 当前 PTY 创建时写的是 `LC_CTYPE=UTF-8`
  - 对 macOS GUI 启动场景而言，这个值过弱且不可靠，容易导致子进程实际 locale 不完整

## 目标

一次性修完下面两件事：

### A. macOS 终端乱码

- PTY 子进程必须拿到可靠的 UTF-8 locale
- 不破坏现有 Unicode11 / CJK fallback fonts
- 避免 shell / python / node / git / TUI 在 macOS GUI 启动下回退到 ASCII / C locale

### B. macOS 右键图片粘贴

- 终端区域真正使用统一的自定义右键菜单
- 菜单点“粘贴”时再触发 `pasteToTerminal()`
- 右键动作本身不能先干扰 Claude/Codex 的 TUI 鼠标交互
- 行为与 `Ctrl+Shift+V` 尽量一致

## 非目标

- 不改版本号
- 不改 README
- 不重构全部 context menu 系统
- 不移除现有 Windows 图片兜底
- 不重新设计剪贴板架构

## 问题一：macOS 乱码的根因

### 现象

用户反馈 macOS 上终端“又出现乱码”。

### 当前最可疑回归点

文件：

- `src-tauri/src/pty.rs`

当前代码：

```rust
cmd.env("LC_CTYPE", "UTF-8");
```

问题：

- 裸的 `UTF-8` 不是稳定可靠的完整 locale 名称
- 对从 GUI 启动的 Tauri app，环境里经常没有 shell 中那套完整 `LANG` / `LC_*`
- 于是子进程可能得到：
  - 没有 `LANG`
  - 一个不完整的 `LC_CTYPE`
  - 最终 shell / 运行时走回 `C` / `US-ASCII` 路径

这会造成：

- 中文乱码
- emoji 宽度 / 输出异常
- 某些 CLI 在 locale 探测时回退到 ASCII

### 为什么不是前端主因

前端 CJK 修复仍然存在：

- `src/utils/terminalCache.ts` 已加载 `Unicode11Addon`
- `src/utils/terminalCache.ts` 已包含：
  - `PingFang SC`
  - `Hiragino Sans GB`
  - `Noto Sans Mono CJK SC`

所以这次优先修 PTY locale，而不是重搞字体。

## 问题二：macOS 右键图片粘贴未真正打通

### 现象

在 dev 跑起来的 Mini-Term 里：

- 用微信截图，点确认后，图片进入系统剪贴板
- 在 Claude/Codex 对话框区域右键
- 卡一会
- TUI 出现长条横线 / 异常视觉变化
- 没有像 `Ctrl+Shift+V` 那样出现 `image1`

### 当前代码里的真实问题

当前存在两套右键逻辑：

#### 外层：`TerminalInstance.tsx`

文件：

- `src/components/TerminalInstance.tsx`

有这套菜单：

```tsx
showContextMenu(e.clientX, e.clientY, [
  { label: '复制', ... },
  { label: '粘贴', onClick: () => { void pasteToTerminal(ptyId); ... } },
]);
```

#### 内层：`terminalCache.ts`

文件：

- `src/utils/terminalCache.ts`

当前 xterm wrapper 还直接做了：

```ts
wrapper.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  e.stopPropagation();
  ...
  void pasteToTerminal(ptyId).finally(() => {
    term.focus();
  });
});
```

### 这会导致什么

- 真正点到终端内容区域时，事件先被内层 wrapper 拦截
- 外层 `showContextMenu(...)` 往往根本不会真正接管该次右键
- 右键动作本身又可能已经先被 TUI 当作鼠标交互处理
- 于是出现：
  - 终端卡顿
  - TUI 出现横线 / UI 变化
  - 但不是稳定的图片粘贴行为

### 结论

当前不是“图片读取逻辑完全没写”，而是：

- **真正生效的右键交互路径不对**
- 终端区域内的右键没有真正走“弹菜单 -> 选择粘贴 -> 再 paste”这条安全路径

## 总体修复策略

### Part A: 修 PTY locale

把当前：

```rust
cmd.env("LC_CTYPE", "UTF-8");
```

改成：

- 若父进程已有 UTF-8 locale，则优先继承
- 若没有，则补一个**完整** locale 名
- macOS 默认兜底建议：
  - `LANG=en_US.UTF-8`
  - `LC_CTYPE=en_US.UTF-8`

### Part B: 统一右键菜单入口

删除 `terminalCache.ts` 中 xterm wrapper 的“立即 paste / 立即 copy”右键逻辑。

保留并统一使用：

- `TerminalInstance.tsx` 的 `showContextMenu(...)`

最终流程应是：

1. 用户在终端区域右键
2. 弹出自定义菜单
3. 用户点“粘贴”
4. 再调用 `pasteToTerminal()`

这样右键事件本身就不会提前扰动 Claude/Codex TUI。

---

## 实施顺序

### Task 1: 修复 macOS PTY locale 注入

**文件：**

- `src-tauri/src/pty.rs`

- [ ] **Step 1: 替换 `LC_CTYPE=UTF-8` 的写死逻辑**

当前代码附近在 PTY 创建时设置：

```rust
cmd.env("TERM", "xterm-256color");
cmd.env("COLORTERM", "truecolor");
cmd.env("LC_CTYPE", "UTF-8");
```

改成“继承现有 UTF-8 locale，否则补默认值”的逻辑。

推荐实现：

```rust
fn has_utf8_locale(value: &str) -> bool {
    value.to_ascii_uppercase().contains("UTF-8")
}
```

在 `create_pty` 内：

```rust
let inherited_lang = std::env::var("LANG").ok();
let inherited_lc_ctype = std::env::var("LC_CTYPE").ok();

let fallback_locale = if cfg!(target_os = "macos") {
    "en_US.UTF-8"
} else {
    "C.UTF-8"
};

if !inherited_lang.as_deref().is_some_and(has_utf8_locale) {
    cmd.env("LANG", fallback_locale);
}

if !inherited_lc_ctype.as_deref().is_some_and(has_utf8_locale) {
    let lc_ctype_value = inherited_lang
        .as_deref()
        .filter(|v| has_utf8_locale(v))
        .unwrap_or(fallback_locale);
    cmd.env("LC_CTYPE", lc_ctype_value);
}
```

**要求：**

- 不要再写死裸 `UTF-8`
- 不要强行覆盖用户已有的 UTF-8 locale
- 但在环境缺失时必须保证 PTY 拿到完整 UTF-8 locale

- [ ] **Step 2: reader 线程断开分支复用 UTF-8 边界处理**

当前 `src-tauri/src/pty.rs` 的 `Disconnected` 分支还有：

```rust
let data = String::from_utf8_lossy(&pending).into_owned();
```

这会在退出瞬间把半个多字节字符替成 `�`。

要求：

- 让 `Disconnected` 分支也走与常规 flush 一样的 UTF-8 边界处理
- 至少不要在尾部制造新的 mojibake

可以抽一个 helper，例如：

```rust
fn split_valid_utf8_prefix(bytes: &[u8]) -> (usize, &[u8], &[u8])
```

或者更简单些，把现有逻辑提成函数复用。

---

### Task 2: 删除 xterm wrapper 级别的直接右键 paste/copy

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 删除/禁用 `wrapper.addEventListener('contextmenu', ...)`**

当前这段是问题根源：

```ts
wrapper.addEventListener('contextmenu', (e) => {
  e.preventDefault();
  e.stopPropagation();
  const sel = term.getSelection();
  if (sel) {
    writeText(sel);
    term.clearSelection();
  } else {
    void pasteToTerminal(ptyId).finally(() => {
      term.focus();
    });
  }
});
```

处理要求：

- 不要再在这里直接 copy / paste
- 不要让内层 wrapper 抢先吃掉终端区域右键

最简单方案：

- 直接删除整段监听

如果担心清理问题，也可以保留结构但改为空操作，不过不推荐。

- [ ] **Step 2: 保留快捷键路径**

保留：

```ts
if (e.ctrlKey && e.shiftKey && e.code === 'KeyV') {
  ...
}
```

快捷键路径不需要改。

---

### Task 3: 统一使用 `TerminalInstance.tsx` 的自定义菜单

**文件：**

- `src/components/TerminalInstance.tsx`

- [ ] **Step 1: 保持 `onContextMenu` 为唯一右键入口**

当前已有：

```tsx
const handleContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
  e.preventDefault();
  const hasSelection = !!getCachedTerminal(ptyId)?.term.getSelection();
  showContextMenu(...)
}
```

要求：

- 这套逻辑成为终端区域唯一有效的右键入口
- 点击菜单项时再执行 copy / paste

- [ ] **Step 2: 菜单点击“粘贴”后保证聚焦**

当前已有：

```ts
onClick: () => {
  void pasteToTerminal(ptyId);
  getCachedTerminal(ptyId)?.term.focus();
}
```

建议微调成：

```ts
onClick: () => {
  void pasteToTerminal(ptyId).finally(() => {
    getCachedTerminal(ptyId)?.term.focus();
  });
}
```

这样与之前内层逻辑一致，更稳。

- [ ] **Step 3: 如有需要，补充事件阻断**

如果删除内层 wrapper 监听后，终端内容区域右键仍被 xterm / TUI 抢占，需要在外层再观察是否要补：

- `onMouseDownCapture`
- `onMouseUpCapture`

但**先不要过度修改**，优先看删除内层监听后是否已恢复正常。

---

### Task 4: 保持现有图片读取路径，不重构

**文件：**

- `src/utils/terminalCache.ts`
- `src-tauri/src/clipboard.rs`

- [ ] **Step 1: 不动现有 5 层粘贴优先级**

保留当前：

1. `readImage()` 像素落盘
2. macOS `NSPasteboard` 兜底
3. Windows `CF_DIB / CF_BITMAP`
4. 纯文本
5. `Alt+V`

本次不要再重构剪贴板策略。

- [ ] **Step 2: 仅修“右键命中的交互路径”**

重点是：

- 快捷键已经基本可用
- 问题在右键入口不是菜单式

---

### Task 5: 验证

- [ ] **Step 1: 前端构建**

```bash
npm run build
```

- [ ] **Step 2: Rust 编译**

```bash
cd src-tauri && cargo check
```

- [ ] **Step 3: macOS 文本编码验证**

在 app 内新开终端，执行：

```bash
locale
python3 - <<'PY'
print("中文😀")
PY
printf '你好，世界\n'
```

预期：

- `LANG` / `LC_CTYPE` 至少其中之一是完整 UTF-8 locale
- 中文与 emoji 显示正常
- 不出现 `�` / 乱码

- [ ] **Step 4: 快捷键图片粘贴验证**

在 Claude/Codex TUI 里：

- 复制一张系统截图
- 用 `Ctrl+Shift+V`
- 预期：仍能得到 `image1` 或等价图片附件行为

- [ ] **Step 5: 右键图片粘贴验证**

同样的剪贴板图片：

- 在终端区域右键
- 应弹出自定义菜单
- 点击“粘贴”
- 预期：行为与快捷键一致
- 不应再出现“卡一会 + TUI 横线”

- [ ] **Step 6: 微信截图验证**

macOS 上用微信截图：

- 截图完成，点确认，图片进入系统剪贴板
- 在 Claude/Codex TUI 中测试：
  - `Ctrl+Shift+V`
  - 右键菜单 -> 粘贴

预期：

- 两者都应表现一致
- 至少不应再被右键事件本身干扰

## 推荐提交拆分

建议拆成 2 个提交：

1. `fix(pty): restore reliable UTF-8 locale for macOS terminal sessions`
2. `fix(terminal): route macOS right-click paste through custom context menu only`

如果只想保留一个提交，也可以。

## 风险点

### 1. 不要动快捷键路径

当前快捷键 `Ctrl+Shift+V` 是相对正确的基线。

本次主要修右键入口，不要把快捷键也一起改坏。

### 2. 不要保留两套同时生效的右键逻辑

必须保证：

- 要么只有 wrapper 右键
- 要么只有 `TerminalInstance` 自定义菜单

当前的问题正是因为“两套都在，但真正命中的是错误那套”。

本次建议统一只保留 `TerminalInstance` 这套。

### 3. locale 修复不要回到粗暴覆盖 `LC_ALL`

不要用：

```rust
cmd.env("LC_ALL", "en_US.UTF-8");
```

除非确认没有更温和的方案。

优先：

- 继承已有 UTF-8
- 缺失时只补 `LANG` / `LC_CTYPE`

## 完成定义

满足以下条件才算完成：

- macOS PTY 会话稳定使用 UTF-8 locale
- 中文 / emoji 不再乱码
- 右键终端区域时真正弹出自定义菜单
- 菜单点击“粘贴”与 `Ctrl+Shift+V` 行为一致
- 微信截图场景下不再出现“卡顿 + 横线 + 无 image1”
- `npm run build` 通过
- `cargo check` 通过

## 建议提交信息

```bash
fix(macos): 修复终端 UTF-8 locale 与右键图片粘贴入口
```
