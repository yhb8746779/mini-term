# macOS 剪贴板图片补强计划

> **For Claude Code / agentic workers:** 本文档是对已完成提交 `194c2e3` 的 follow-up 修复，不要重做整套图片粘贴功能；只补齐“macOS / 非 Windows 标准图片内容”和“macOS 私有剪贴板格式兜底”这两块缺口。

## 背景

当前仓库已经在 `194c2e3` 中完成了第一版图片粘贴支持：

- `Ctrl+Shift+V` 与右键粘贴统一走 `pasteToTerminal()`
- Windows 下增加了 `CF_DIB / CF_BITMAP` 读取能力
- 启动时会清理旧的临时 PNG

但目前存在一个关键缺口：

- `src/utils/terminalCache.ts` 里的 `readImage()` 只被当作“有没有图片”的探针
- 真正取图时调用的是 Rust command `read_clipboard_image`
- 该 command 在 `src-tauri/src/clipboard.rs` 中仅 Windows 实现
- 因此 macOS / Linux 即使剪贴板里确实有图片内容，也会直接落到 `Alt+V` fallback

这会导致：

- 普通截图工具在 macOS 上可能“检测到有图，但粘贴不进去”
- 微信截图这类“图片只在剪贴板里，不在文件系统里”的场景，当前实现并未真正覆盖

## 目标

修复后需要覆盖这两类需求：

1. **标准图片内容在剪贴板里**
   - 例如系统截图、微信截图、浏览器复制图片
   - 即便不是文件系统里的图片，也能读出像素数据
   - 保存到临时 PNG 后把路径写入 PTY

2. **macOS 私有/非标准 pasteboard 图片格式**
   - 如果 `readImage()` 读不到，但系统剪贴板里其实有图片
   - 通过 macOS 原生 `NSPasteboard` 再兜底一次

## 明确的行为目标

修复后 `pasteToTerminal()` 应遵循以下顺序：

1. **跨平台主路径：前端 `readImage()` 真实取图**
   - 如果成功拿到 `Image`
   - 直接取 `rgba()` 和 `size()`
   - 发给 Tauri 存成临时 PNG
   - 返回路径写入 PTY

2. **平台原生兜底**
   - Windows：继续保留现有 `CF_DIB / CF_BITMAP`
   - macOS：新增 `NSPasteboard` 图片读取兜底

3. **最后退回 `Alt+V`**
   - 仅在上述路径都失败时触发

## 非目标

- 不回滚或重写 `194c2e3`
- 不改版本号
- 不改 README
- 不重构终端键盘逻辑
- 不移除 Windows 兜底逻辑

## 当前问题定位

### `src/utils/terminalCache.ts`

当前逻辑大致是：

```ts
if (await clipboardHasImage()) {
  try {
    const path: string = await invoke('read_clipboard_image');
    await enqueuePtyWrite(ptyId, path);
    return;
  } catch {}

  await enqueuePtyWrite(ptyId, '\x1bv');
  return;
}
```

问题在于：

- `clipboardHasImage()` 通过 `readImage()` 检测到图片后
- 并没有继续使用 `readImage()` 得到的图片内容
- 而是切到 Windows-only 的 Rust 命令

所以 macOS 上当前是：

- 能“发现图片”
- 但不能“把图片保存成文件”

## 总体方案

### 第一层：跨平台标准图片保存

新增一个 Tauri command，用于把前端拿到的 RGBA 数据保存成 temp PNG：

```rust
#[tauri::command]
fn save_clipboard_rgba_image(rgba: Vec<u8>, width: u32, height: u32) -> Result<String, String>
```

职责：

- 接收前端传来的原始 RGBA 像素
- 保存到 `temp_dir()/mini-term-clipboard/clip-<millis>.png`
- 返回文件路径

前端改成：

1. `const image = await readImage()`
2. `const rgba = await image.rgba()`
3. `const { width, height } = await image.size()`
4. `invoke('save_clipboard_rgba_image', { rgba: Array.from(rgba), width, height })`

这样即使图片只是存在系统剪贴板里，不是文件系统里的图片，也能落地成临时 PNG。

### 第二层：macOS 原生 `NSPasteboard` 兜底

如果前端 `readImage()` 失败，且平台是 macOS，再调用新的 Tauri command：

```rust
#[tauri::command]
fn read_clipboard_image_macos() -> Result<String, String>
```

行为：

- 使用 `NSPasteboard::generalPasteboard`
- 尝试读取：
  - `NSImage`
  - TIFF data
  - PNG data
- 成功后转换/保存为 temp PNG
- 返回路径

这样可以提高对微信截图、某些 IM / 截图工具私有格式的兼容性。

### 第三层：保留现有 Windows 兜底

现有：

- `read_clipboard_image()`（Windows）
- `CF_DIB / CF_BITMAP`

继续保留，不动现有能力。

## 涉及文件

**修改：**

- `src/utils/terminalCache.ts`
- `src-tauri/src/clipboard.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

**可能新增依赖：**

- macOS 原生桥接依赖，按当前项目 Tauri 生态选最小方案

## 实施顺序

---

### Task 1: 前端把 `readImage()` 从“探针”升级成“主数据源”

**文件：**

- `src/utils/terminalCache.ts`

- [ ] **Step 1: 拆分探测与取图逻辑**

当前 `clipboardHasImage()` 的思路要改。

目标不是“先问有没有图”，而是“直接尝试读取图”：

```ts
async function readClipboardImageData(): Promise<{
  rgba: Uint8Array;
  width: number;
  height: number;
} | null> {
  try {
    const image = await readImage();
    const [rgba, size] = await Promise.all([image.rgba(), image.size()]);
    return { rgba, width: size.width, height: size.height };
  } catch {
    return null;
  }
}
```

**注意：**

- `readImage()` 成功时，说明标准图片路径已经可用
- 不要再仅仅把它用于 `boolean` 检测

- [ ] **Step 2: `pasteToTerminal()` 调整为 4 层顺序**

推荐逻辑：

```ts
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const image = await readClipboardImageData();
  if (image) {
    try {
      const path: string = await invoke('save_clipboard_rgba_image', {
        rgba: Array.from(image.rgba),
        width: image.width,
        height: image.height,
      });
      await enqueuePtyWrite(ptyId, path);
      return;
    } catch {
      // 继续走平台原生 fallback
    }
  }

  try {
    const path: string = await invoke('read_clipboard_image');
    await enqueuePtyWrite(ptyId, path);
    return;
  } catch {
    // Windows/macOS 原生读取失败，继续后续 fallback
  }

  const text = await readText().catch(() => null);
  if (text) {
    await enqueuePtyWrite(ptyId, text);
    return;
  }

  await enqueuePtyWrite(ptyId, '\x1bv');
}
```

**但要按平台微调：**

- `save_clipboard_rgba_image` 是跨平台主路径
- `read_clipboard_image` 不应再只是 Windows-only 的唯一主路径
- 如新增 `read_clipboard_image_macos`，应在 mac 上单独尝试它

更推荐的最终顺序：

1. `save_clipboard_rgba_image`（基于 `readImage()`）
2. mac: `read_clipboard_image_macos`
3. windows: `read_clipboard_image`
4. `readText()`
5. `Alt+V`

- [ ] **Step 3: 右键菜单继续复用 `pasteToTerminal()`**

保持当前：

```ts
void pasteToTerminal(ptyId).finally(() => {
  term.focus();
});
```

不要退回分叉逻辑。

---

### Task 2: Rust 增加跨平台 PNG 落盘 command

**文件：**

- `src-tauri/src/clipboard.rs`

- [ ] **Step 1: 提炼公共保存 helper**

当前 `save_png(rgba, width, height)` 在 Windows 模块里。

需要把“保存 RGBA 到 temp PNG”的逻辑提到模块公共层，供：

- Windows 原生读取结果
- 前端 `readImage()` 结果
- macOS 原生读取结果

共同复用。

可以整理成：

```rust
fn save_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<PathBuf, String>
```

- [ ] **Step 2: 新增 command**

新增：

```rust
#[tauri::command]
pub fn save_clipboard_rgba_image(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "图片尺寸溢出".to_string())?;

    if rgba.len() != expected as usize {
        return Err(format!(
            "RGBA 长度不匹配: got {}, expected {}",
            rgba.len(),
            expected
        ));
    }

    let path = save_rgba_png(&rgba, width, height)?;
    Ok(path.to_string_lossy().into_owned())
}
```

**要求：**

- 跨平台可用
- 只负责落盘，不负责读系统剪贴板

---

### Task 3: macOS 增加 `NSPasteboard` 原生兜底

**文件：**

- `src-tauri/src/clipboard.rs`
- `src-tauri/Cargo.toml`

- [ ] **Step 1: 新增 macOS 模块**

结构建议：

```rust
#[cfg(target_os = "macos")]
mod mac {
    pub fn read_clipboard_to_png() -> Result<PathBuf, String> {
        // NSPasteboard 读取
    }
}
```

- [ ] **Step 2: 优先读标准 NSImage / TIFF / PNG**

可以按以下思路：

- 取 `NSPasteboard::generalPasteboard`
- 优先看能否拿到 `NSImage`
- 或读取 `public.tiff` / `public.png`
- 拿到字节后解码为 RGBA
- 复用公共 `save_rgba_png()`

**关键点：**

- 目标是兼容“微信截图直接放进 pasteboard”
- 不要求保留原始格式
- 最终统一输出 PNG 即可

- [ ] **Step 3: 暴露 command**

新增：

```rust
#[tauri::command]
pub fn read_clipboard_image_macos() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let path = mac::read_clipboard_to_png()?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("仅支持 macOS 平台".into())
    }
}
```

---

### Task 4: 保留并整理 Windows 路径

**文件：**

- `src-tauri/src/clipboard.rs`

- [ ] **Step 1: 现有 `read_clipboard_image()` 继续保留**

不要删：

- `CF_DIB`
- `CF_BITMAP`
- 启动时清理 temp 文件

- [ ] **Step 2: 公共保存逻辑复用**

Windows 路径最终也应复用提炼后的：

```rust
save_rgba_png(...)
```

避免保存逻辑分叉。

---

### Task 5: 注册新 command

**文件：**

- `src-tauri/src/lib.rs`

- [ ] **Step 1: 在 `generate_handler![]` 注册**

新增：

```rust
clipboard::save_clipboard_rgba_image,
clipboard::read_clipboard_image_macos,
```

保留现有：

```rust
clipboard::read_clipboard_image,
```

---

### Task 6: Cargo 依赖处理

**文件：**

- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

- [ ] **Step 1: 根据 macOS 方案增加最小依赖**

原则：

- 只加完成 `NSPasteboard` 所需的最小依赖
- 避免引入大而杂的 GUI 依赖集

如果可以只靠已存在生态 + `image` 完成，优先最小改动。

- [ ] **Step 2: 更新 lockfile**

运行：

```bash
cd src-tauri && cargo check
```

不要手改 `Cargo.lock`。

## 参考伪代码

### 前端最终建议实现

```ts
async function trySaveStandardClipboardImage(): Promise<string | null> {
  try {
    const image = await readImage();
    const [rgba, size] = await Promise.all([image.rgba(), image.size()]);
    return await invoke('save_clipboard_rgba_image', {
      rgba: Array.from(rgba),
      width: size.width,
      height: size.height,
    });
  } catch {
    return null;
  }
}

export async function pasteToTerminal(ptyId: number): Promise<void> {
  const standardImagePath = await trySaveStandardClipboardImage();
  if (standardImagePath) {
    await enqueuePtyWrite(ptyId, standardImagePath);
    return;
  }

  try {
    const path: string = await invoke('read_clipboard_image_macos');
    await enqueuePtyWrite(ptyId, path);
    return;
  } catch {}

  try {
    const path: string = await invoke('read_clipboard_image');
    await enqueuePtyWrite(ptyId, path);
    return;
  } catch {}

  const text = await readText().catch(() => null);
  if (text) {
    await enqueuePtyWrite(ptyId, text);
    return;
  }

  await enqueuePtyWrite(ptyId, '\x1bv');
}
```

**实际实现时要根据平台判断，避免无意义 invoke：**

- macOS 才调 `read_clipboard_image_macos`
- Windows 才调 `read_clipboard_image`

平台判断可以优先用前端 runtime 平台信息，或直接让 invoke 失败后吞掉，但推荐显式判断，日志更干净。

## 验证清单

- [ ] `Ctrl+Shift+V` 粘贴普通文本正常
- [ ] 右键无选中时粘贴普通文本正常
- [ ] 右键有选中时仍然执行复制
- [ ] macOS 系统截图进入剪贴板后可落成 temp PNG 路径
- [ ] macOS 微信上的截图进入剪贴板后，若 `readImage()` 可读，则能正常落盘
- [ ] 若微信截图 `readImage()` 不可读，则 `NSPasteboard` 兜底仍能成功
- [ ] Windows 现有 PinPix / 非标准截图格式能力不回退
- [ ] temp 目录清理逻辑继续有效
- [ ] `npm run build` 通过
- [ ] `cd src-tauri && cargo check` 通过

## 完成定义

满足以下条件才算修复完成：

- 图片即使只存在于系统剪贴板，不在文件系统中，也能在 macOS 上被保存成 temp PNG
- 右键粘贴与 `Ctrl+Shift+V` 行为一致
- 微信截图这类来源至少覆盖“标准图片路径”，并对私有 pasteboard 格式提供 macOS 原生兜底
- Windows 现有兜底不受影响
- 最终失败时仍保留 `Alt+V` 作为最后保险

## 建议提交信息

```bash
fix(clipboard): 补齐 macOS 剪贴板图片落盘与 pasteboard 兜底
```
