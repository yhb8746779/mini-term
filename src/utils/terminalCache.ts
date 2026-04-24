/**
 * 终端实例缓存：在 React 组件卸载/重新挂载期间保持 xterm.js Terminal 存活。
 *
 * 问题：分屏操作导致 SplitLayout 从 leaf 变为 split 节点，React 会卸载旧的
 * TerminalInstance 并重建新的，xterm.js 实例被 dispose，终端内容丢失。
 *
 * 方案：Terminal 实例按 ptyId 缓存。组件 mount 时附着 wrapper 到容器，
 * unmount 时仅分离 wrapper，不销毁 Terminal。仅在面板真正关闭时调用 dispose。
 */

import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { WebglAddon } from '@xterm/addon-webgl';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { readText, readImage, writeText } from '@tauri-apps/plugin-clipboard-manager';
import { useAppStore, findPaneByPty } from '../store';
import type { PtyOutputPayload, AiProvider } from '../types';
import { getResolvedTheme } from './themeManager';
import { createPtyWriteQueue } from './ptyWriteQueue';

export interface CachedTerminal {
  term: Terminal;
  fitAddon: FitAddon;
  wrapper: HTMLDivElement;
}

interface CachedEntry extends CachedTerminal {
  cleanup: () => void;
}

export const DARK_TERMINAL_THEME = {
  background: '#100f0d',
  foreground: '#d8d4cc',
  cursor: '#c8805a',
  cursorAccent: '#100f0d',
  selectionBackground: '#c8805a30',
  selectionForeground: '#e5e0d8',
  black: '#2a2824',
  red: '#d4605a',
  green: '#6bb87a',
  yellow: '#d4a84a',
  blue: '#6896c8',
  magenta: '#b08cd4',
  cyan: '#7dcfb8',
  white: '#d8d4cc',
  brightBlack: '#5c5850',
  brightRed: '#e07060',
  brightGreen: '#80d090',
  brightYellow: '#e0b860',
  brightBlue: '#80aad8',
  brightMagenta: '#c0a0e0',
  brightCyan: '#90e0c8',
  brightWhite: '#e5e0d8',
};

export const LIGHT_TERMINAL_THEME = {
  background: '#fafafa',
  foreground: '#1a1a1a',
  cursor: '#b06830',
  cursorAccent: '#fafafa',
  selectionBackground: '#b0683030',
  selectionForeground: '#1a1a1a',
  black: '#1a1a1a',
  red: '#c0392b',
  green: '#2d8a46',
  yellow: '#b08620',
  blue: '#2860a0',
  magenta: '#8a5cb8',
  cyan: '#1a8a6a',
  white: '#808080',
  brightBlack: '#666666',
  brightRed: '#e04030',
  brightGreen: '#38a058',
  brightYellow: '#c89830',
  brightBlue: '#3870b8',
  brightMagenta: '#a070d0',
  brightCyan: '#28a080',
  brightWhite: '#a0a0a0',
};

export function getTerminalTheme(terminalFollowTheme: boolean): typeof DARK_TERMINAL_THEME {
  if (terminalFollowTheme && getResolvedTheme() === 'light') {
    return LIGHT_TERMINAL_THEME;
  }
  return DARK_TERMINAL_THEME;
}

const cache = new Map<number, CachedEntry>();
const enqueuePtyWrite = createPtyWriteQueue((ptyId, data) =>
  invoke('write_pty', { ptyId, data })
);

// ── 诊断开关 & 日志 ─────────────────────────────────────────────────────────
// 用法：DevTools Console → localStorage.setItem('mini-term-debug', '1')
//       调试完 → localStorage.removeItem('mini-term-debug')
const TERM_DEBUG = localStorage.getItem('mini-term-debug') === '1';
const FORCE_DISABLE_WEBGL = localStorage.getItem('mini-term-disable-webgl') === '1';
const FORCE_MONO_ONLY = localStorage.getItem('mini-term-mono-only') === '1';

function debugTerm(scope: string, payload: Record<string, unknown>) {
  if (!TERM_DEBUG) return;
  console.info(`[mini-term-debug] ${scope}`, payload);
}

export function getOrCreateTerminal(ptyId: number): CachedTerminal {
  const existing = cache.get(ptyId);
  if (existing) return existing;

  // 创建 wrapper 容器，xterm.js 会在其中渲染
  const wrapper = document.createElement('div');
  wrapper.style.width = '100%';
  wrapper.style.height = '100%';

  // 字体栈策略：
  //   1) 系统自带的等宽字体放在最前（macOS=Menlo/SF Mono，Windows=Consolas，Linux=Liberation/DejaVu）。
  //      这样 xterm 首次测量 cell-width 一定命中 Latin 等宽字体，避免回退到 CJK 全角
  //      字体（PingFang SC 等）导致 cell 偏宽、字符间留大空隙的"丑字距"现象。
  //   2) 用户装了第三方等宽字体（JetBrains Mono / Cascadia Code）作为可选升级。
  //   3) CJK 回退字体放在最后，仅在遇到中文时生效，不参与 cell-width 测量。
  const fontFamily = FORCE_MONO_ONLY
    ? "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Cascadia Code', 'JetBrains Mono', 'Liberation Mono', monospace"
    : "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Cascadia Code', 'JetBrains Mono', 'Liberation Mono', 'PingFang SC', 'Hiragino Sans GB', 'Noto Sans Mono CJK SC', 'Microsoft YaHei Mono', monospace";

  const term = new Terminal({
    fontSize: useAppStore.getState().config.terminalFontSize ?? 14,
    // CJK 备用字体：PingFang SC（macOS）/ Noto Sans Mono CJK SC（Linux）/ Microsoft YaHei（Windows）
    // 确保中文字符有合适的字形，避免宽度计算与实际渲染不一致导致乱码
    fontFamily,
    fontWeight: '400',
    fontWeightBold: '600',
    cursorBlink: true,
    cursorStyle: 'bar',
    cursorWidth: 2,
    scrollback: 100000,
    letterSpacing: 0,
    lineHeight: 1.35,
    theme: getTerminalTheme(useAppStore.getState().config.terminalFollowTheme ?? true),
  });

  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);

  // 拦截 CSI 3J (ED3 - Erase Saved Lines)：保留 scrollback 缓冲区。
  // codex/claude 等 TUI 应用在主缓冲区周期性发送此序列清空滚动历史，
  // 导致用户向上滚动时看不到之前的对话内容。返回 true 让 xterm.js
  // 跳过默认（清空 scrollback）行为；其余 Ps 值（0/1/2）走默认逻辑。
  term.parser.registerCsiHandler({ final: 'J' }, (params) => params[0] === 3);

  term.open(wrapper);

  // Unicode 11 addon：修正 CJK / Emoji 双宽字符的列宽计算，避免中文乱码
  try {
    const unicode11 = new Unicode11Addon();
    term.loadAddon(unicode11);
    term.unicode.activeVersion = '11';
  } catch {
    // Unicode 11 不支持
  }

  // WebGL 渲染，降级时回退到 Canvas
  if (FORCE_DISABLE_WEBGL) {
    debugTerm('terminal:webgl_skipped', { ptyId, reason: 'FORCE_DISABLE_WEBGL' });
  } else {
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        debugTerm('terminal:webgl_context_loss', { ptyId });
        webgl.dispose();
        term.refresh(0, term.rows - 1);
      });
      term.loadAddon(webgl);
      debugTerm('terminal:webgl_loaded', { ptyId });
    } catch (err) {
      debugTerm('terminal:webgl_failed', { ptyId, error: String(err) });
    }
  }

  debugTerm('terminal:create', {
    ptyId,
    fontFamily: term.options.fontFamily,
    fontSize: term.options.fontSize,
    lineHeight: term.options.lineHeight,
    letterSpacing: term.options.letterSpacing,
    webgl: !FORCE_DISABLE_WEBGL,
    monoOnly: FORCE_MONO_ONLY,
  });

  // 字体加载稳定兜底：首次测量 cell-width 可能发生在字体还没完全就绪时，
  // xterm/WebGL 会把测到的（偏大的）cell 尺寸烧进纹理图集，造成字符间距突兀。
  // document.fonts.ready 在所有 @font-face 解析完成后触发；此时再跑一次
  // fit + refresh，WebGL 纹理图集会按当前生效的字体重建，字距恢复正常。
  if (typeof document !== 'undefined' && document.fonts && document.fonts.ready) {
    document.fonts.ready
      .then(() => {
        try {
          fitAddon.fit();
          term.refresh(0, Math.max(term.rows - 1, 0));
          invoke('resize_pty', { ptyId, cols: term.cols, rows: term.rows }).catch(() => {});
        } catch {
          // terminal 可能已被 dispose（pty 关闭）；忽略
        }
      })
      .catch(() => {});
  }

  // 剪贴板快捷键 + macOS WKWebView Ctrl 键修复
  // macOS 的 WKWebView 对 Ctrl+A/E/K/U/W 等有系统级文本编辑绑定（继承自 NeXTSTEP），
  // 会在 OS 层面干扰 xterm.js 隐藏 textarea 的输入，导致这些控制字符无法正确送到 PTY。
  // 系统 Terminal.app 是原生应用不受影响，但 Mini-Term 需要显式拦截并手动发送。
  const MACOS_CTRL_MAP: Partial<Record<string, string>> = {
    KeyA: '\x01', KeyB: '\x02', KeyE: '\x05', KeyF: '\x06',
    KeyK: '\x0b', KeyL: '\x0c', KeyN: '\x0e', KeyP: '\x10',
    KeyU: '\x15', KeyW: '\x17', KeyY: '\x19',
  };
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== 'keydown') return true;
    if (e.ctrlKey && e.shiftKey && e.code === 'KeyC') {
      e.preventDefault();
      void copyTerminalSelection(ptyId);
      return false;
    }
    if (e.ctrlKey && e.shiftKey && e.code === 'KeyV') {
      e.preventDefault();
      void pasteToTerminal(ptyId);
      return false;
    }
    // macOS 只在 Ctrl 单独按下时（无 Shift/Meta/Alt）才有系统绑定干扰
    if (e.ctrlKey && !e.shiftKey && !e.metaKey && !e.altKey) {
      const data = MACOS_CTRL_MAP[e.code];
      if (data) {
        e.preventDefault();
        void enqueuePtyWrite(ptyId, data);
        return false;
      }
    }
    return true;
  });

  // 右键菜单统一由 TerminalInstance.tsx 的 onContextMenu 处理，
  // 此处不再拦截，避免与外层自定义菜单冲突并干扰 TUI 鼠标交互。

  // 用户输入 → PTY
  const onDataDisp = term.onData((data) => {
    term.scrollToBottom();
    void enqueuePtyWrite(ptyId, data);
  });

  // 终端 resize → 同步到 PTY
  const onResizeDisp = term.onResize(({ cols, rows }) => {
    invoke('resize_pty', { ptyId, cols, rows });
  });

  // PTY 输出 → 终端
  let cancelled = false;
  let unlisten: (() => void) | undefined;
  listen<PtyOutputPayload>('pty-output', (event) => {
    if (event.payload.ptyId === ptyId) {
      term.write(event.payload.data);
    }
  }).then((fn) => {
    if (cancelled) fn();
    else unlisten = fn;
  });

  const cleanup = () => {
    cancelled = true;
    unlisten?.();
    onDataDisp.dispose();
    onResizeDisp.dispose();
    term.dispose();
  };

  const entry: CachedEntry = { term, fitAddon, wrapper, cleanup };
  cache.set(ptyId, entry);
  return entry;
}

/** 获取已缓存的终端（不创建新的） */
export function getCachedTerminal(ptyId: number): CachedTerminal | undefined {
  return cache.get(ptyId);
}

/** 彻底销毁终端（面板关闭 / kill_pty 后调用） */
export function disposeTerminal(ptyId: number): void {
  const entry = cache.get(ptyId);
  if (!entry) return;
  entry.wrapper.remove();
  entry.cleanup();
  cache.delete(ptyId);
}

export function updateAllTerminalThemes(terminalFollowTheme: boolean): void {
  const theme = getTerminalTheme(terminalFollowTheme);
  for (const entry of cache.values()) {
    entry.term.options.theme = theme;
  }
}

export function writePtyInput(ptyId: number, data: string): Promise<void> {
  return enqueuePtyWrite(ptyId, data);
}

/**
 * 获取当前选中文本：优先取 xterm 选区，fallback 到页面 DOM 选区。
 * 解决 Claude/Codex TUI 内部自管理文本区域不反映为 xterm selection 的问题。
 */
export function getAnySelectedText(ptyId: number): string {
  const termSel = cache.get(ptyId)?.term.getSelection() ?? '';
  if (termSel) return termSel;
  return window.getSelection()?.toString().trim() ?? '';
}

/** 把指定文本写入系统剪贴板。供右键菜单在弹出时已拿到快照的场景使用。 */
export async function copyTextToClipboard(text: string): Promise<boolean> {
  if (!text) return false;
  try {
    await writeText(text);
    return true;
  } catch {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      return false;
    }
  }
}

/** 复制当前选中文本到系统剪贴板（供快捷键 Ctrl+Shift+C 调用）。 */
export async function copyTerminalSelection(ptyId: number): Promise<boolean> {
  return copyTextToClipboard(getAnySelectedText(ptyId));
}

export const _isMacOS = /Mac OS X|Macintosh/.test(navigator.userAgent);
export const _isWindows = /Windows/.test(navigator.userAgent);
export const _isLinux = /Linux/.test(navigator.userAgent) && !_isWindows && !_isMacOS;

/** 把剪贴板图片落盘成临时 PNG，返回路径；任何环节失败返回 null */
async function trySaveStandardClipboardImage(): Promise<string | null> {
  try {
    const image = await readImage();
    const [rgba, size] = await Promise.all([image.rgba(), image.size()]);
    const path: string = await invoke('save_clipboard_rgba_image', {
      rgba: Array.from(rgba),
      width: size.width,
      height: size.height,
    });
    return path;
  } catch {
    return null;
  }
}

/**
 * 返回 ptyId 对应 pane 的 AI provider（仅在 AI 活跃状态时）。
 * 非 AI pane 或状态为 idle/error 时返回 null。
 */
function getAiProviderForPty(ptyId: number): AiProvider | null {
  const { projectStates } = useAppStore.getState();
  for (const ps of projectStates.values()) {
    for (const tab of ps.tabs) {
      const pane = findPaneByPty(tab.splitLayout, ptyId);
      if (pane) {
        const isAi =
          pane.status === 'ai-generating' ||
          pane.status === 'ai-thinking' ||
          pane.status === 'ai-complete' ||
          pane.status === 'ai-awaiting-input';
        return isAi ? (pane.aiProvider ?? null) : null;
      }
    }
  }
  return null;
}

// ── AI pane 剪贴板来源分类与粘贴路径 ──────────────────────────────────────────
//
// 剪贴板来源必须区分为以下几类，三种图片相关类型不可互换：
//
//   1. plain-text           → sendAiTextPaste
//   2. raw-image            → sendAiScreenshotImagePaste      （截图工具图片位图）
//   3. explorer-image-files → sendAiExplorerImageFilesPaste   （Explorer 复制的图片文件路径）
//   4. explorer-files       → sendAiExplorerFilesPaste        （Explorer 复制的普通文件路径）
//   5. finder-image-files   → sendAiExplorerImageFilesPaste   （Finder 复制的图片文件路径）
//   6. finder-files         → sendAiExplorerFilesPaste        （Finder 复制的普通文件路径）
//   7. rich-object          → Ctrl+V fallback                 （无法识别的富剪贴板对象）
//
// ─── 三种"图片相关"类型的本质区别 ─────────────────────────────────────────────
//
//   raw-image
//     = 截图工具/图片编辑器直接写入剪贴板的图片位图（CF_DIB/CF_BITMAP）。
//       Web Clipboard API 暴露 image/* MIME type。
//       Mini-Term 增强路径：Windows Alt+V 让 Claude/Codex 从剪贴板读取真实图片数据。
//
//   explorer-image-files / finder-image-files
//     = 资源管理器/Finder 复制的图片文件（CF_HDROP / public.file-url 文件路径列表）。
//       Rust 后端按扩展名判定（.png/.jpg/.gif 等）。
//       本质是文件路径引用，不是图片位图 → 不能走 Alt+V，否则报 "no image" 错误。
//       必须走专属路径 sendAiExplorerImageFilesPaste，与普通文件保持代码意图隔离。
//
//   explorer-files / finder-files
//     = 资源管理器/Finder 复制的普通文件（CF_HDROP，非图片扩展名）。
//       路径文本注入，Claude Code 自行处理文件引用。
//
//   rich-object
//     = Web API 可见的富对象但后端无法提取文件路径，Ctrl+V fallback。
//
// ─── 混用这些类型会导致 ────────────────────────────────────────────────────────
//   - Explorer 文件对象走 Alt+V → "no image" 错误
//   - 截图图片走文本路径 → 图片内容丢失
//   - explorer-image-files 与 explorer-files 共用同一函数 → 无法针对图片文件做独立优化

/**
 * AI pane 文本粘贴路径。
 *
 * term.paste() 内部包装 bracketed-paste 序列 \x1b[200~...\x1b[201~，
 * 让 Claude/Codex/Gemini CLI 识别为粘贴块（[Pasted text #1 +N lines]）。
 *
 * 为什么不用 enqueuePtyWrite 直接注入 bracketed-paste 字符串：
 *   直接注入绕过 xterm paste 管道，TUI CLI 不会识别为"粘贴事件"。
 *
 * 为什么要先 focus：
 *   右键等操作可能使焦点从 xterm 漂移，term.paste() 在失焦时静默失败。
 */
function sendAiTextPaste(ptyId: number, text: string): void {
  const entry = cache.get(ptyId);
  if (!entry) return;
  entry.term.focus();
  entry.term.paste(text);
}

/**
 * AI pane 截图图片粘贴路径（仅用于真正的图片位图剪贴板内容）。
 *
 * Windows      → Alt+V（\x1bv）：Claude/Codex 图片粘贴专用快捷键。
 * macOS+claude → Ctrl+V（\x16）：WKWebView 剪贴板访问权限足以让 Claude 读取图片。
 * macOS+codex  → Alt+V（\x1bv）：Codex on mac 需要 Alt+V，与 Windows 一致。
 * Linux        → Ctrl+V（\x16）。
 *
 * 重要限制：
 *   - 仅在剪贴板内容确实是图片位图时使用（raw-image 分类）
 *   - 禁止对 Explorer/Finder 文件对象使用，否则报"no image"错误
 *   - 禁止对纯文本使用
 */
async function sendAiScreenshotImagePaste(ptyId: number, provider: AiProvider | null): Promise<void> {
  let key: string;
  let platform: string;
  if (_isWindows) {
    key = 'alt+v'; platform = 'windows';
    await enqueuePtyWrite(ptyId, '\x1bv');
  } else if (_isMacOS) {
    platform = 'mac';
    // Codex on mac 与 Windows 一致用 Alt+V；Claude/Gemini 用 Ctrl+V
    if (provider === 'codex') {
      key = 'alt+v';
      await enqueuePtyWrite(ptyId, '\x1bv');
    } else {
      key = 'ctrl+v';
      await enqueuePtyWrite(ptyId, '\x16');
    }
  } else {
    key = 'ctrl+v'; platform = 'linux';
    await enqueuePtyWrite(ptyId, '\x16');
  }
  debugTerm('paste:image_shortcut', { ptyId, provider, key: key!, platform: platform! });
}

/**
 * AI pane 普通文件路径粘贴路径（explorer-files / finder-files）。
 *
 * 用于已经从 CF_HDROP / public.file-url 提取出路径的非图片文件情形。
 * 路径由 detectClipboardPayload 在分类时提前读取，此处只做格式化注入。
 *
 * 策略：
 *   - 路径含空格时加双引号，多个路径用空格分隔（与原生 PowerShell 粘贴行为一致）
 *   - 通过 sendAiTextPaste 触发 bracketed-paste，Claude Code 展示文件引用
 *
 * 为什么不用 sendAiScreenshotImagePaste（Alt+V）：
 *   CF_HDROP 是文件路径引用，不是图片位图（CF_DIB/CF_BITMAP）。
 *   Alt+V 要求剪贴板中有真实图片数据，文件引用会导致 "no image" 错误。
 *
 * 注意：图片文件路径请使用 sendAiExplorerImageFilesPaste，两者不可混用。
 */
function sendAiExplorerFilesPaste(ptyId: number, paths: string[]): void {
  // 含空格的路径加双引号，多个路径用空格分隔
  const text = paths.map((p) => (p.includes(' ') ? `"${p}"` : p)).join(' ');
  sendAiTextPaste(ptyId, text);
}

/**
 * AI pane 图片文件粘贴路径（explorer-image-files / finder-image-files）。
 *
 * 专用于从资源管理器/Finder 复制的图片文件（.png/.jpg/.gif 等扩展名）。
 * 与 sendAiExplorerFilesPaste（普通文件）严格分离，不可共用同一函数。
 *
 * ─── 策略：load_image_to_clipboard → Alt+V / Ctrl+V ─────────────────────────
 *   1. 调 Rust `load_image_to_clipboard(path)` 将图片文件内容写入系统剪贴板为位图
 *      Windows → CF_DIB；macOS → NSPasteboard TIFF
 *   2. 调 `sendAiScreenshotImagePaste`，与截图粘贴完全一致
 *      Windows/Codex → Alt+V；macOS+Claude → Ctrl+V
 *   AI CLI 从剪贴板读取真实图片位图，以图片块（[Pasted image #N]）展示。
 *
 * ─── 为什么不直接走 Alt+V（不经过此步骤）───────────────────────────────────────
 *   资源管理器复制的文件放的是 CF_HDROP（路径引用），不是 CF_DIB（位图）。
 *   直接 Alt+V 会让 AI CLI 读到 CF_HDROP，报 "no image" 错误。
 *   此函数先把文件内容解码为位图写入剪贴板，再触发 Alt+V，规避该问题。
 *
 * ─── 多图片处理 ──────────────────────────────────────────────────────────────
 *   多张图片依次加载→触发，AI CLI 按顺序产生多个图片块。
 *   若某张图片加载失败，降级为路径文本注入（不中断其余图片）。
 */
async function sendAiExplorerImageFilesPaste(
  ptyId: number,
  paths: string[],
  provider: AiProvider | null,
): Promise<void> {
  for (const path of paths) {
    try {
      // 将图片文件写入剪贴板位图（CF_DIB / NSPasteboard TIFF）
      await invoke('load_image_to_clipboard', { path });
      // 触发 AI CLI 图片粘贴快捷键（与截图路径完全一致）
      await sendAiScreenshotImagePaste(ptyId, provider);
    } catch {
      // 降级：路径文本注入（至少让 AI CLI 能按路径加载图片）
      sendAiTextPaste(ptyId, path.includes(' ') ? `"${path}"` : path);
    }
  }
}

// ── 剪贴板内容分类器 ────────────────────────────────────────────────────────────

type ClipboardPayloadKind =
  | 'plain-text'
  | 'raw-image'
  | 'explorer-image-files'  // Explorer 复制的图片文件（CF_HDROP，扩展名为图片）
  | 'explorer-files'        // Explorer 复制的普通文件（CF_HDROP，扩展名非图片）
  | 'finder-image-files'    // macOS Finder 复制的图片文件（public.file-url，扩展名为图片）
  | 'finder-files'          // macOS Finder 复制的普通文件（public.file-url）
  | 'rich-object'           // Web API 可见但无法提取文件路径的富对象（residual fallback）
  | 'empty-or-unknown';

interface ClipboardPayload {
  kind: ClipboardPayloadKind;
  text?: string;
  paths?: string[];  // explorer-image-files / explorer-files 时携带的文件路径列表
}

/** 图片文件扩展名集合，用于区分 explorer-image-files 和 explorer-files */
const IMAGE_FILE_EXTENSIONS = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.webp',
  '.bmp', '.svg', '.ico', '.tif', '.tiff',
]);

function isImageFilePath(p: string): boolean {
  const lower = p.toLowerCase();
  const dot = lower.lastIndexOf('.');
  if (dot < 0) return false;
  return IMAGE_FILE_EXTENSIONS.has(lower.slice(dot));
}

/**
 * mac 上视为"文本类"的 MIME / UTI 类型集合。
 * 包含这些类型的 ClipboardItem 应优先走 plain-text 路径，
 * 而非误判为 rich-object（mac 应用复制文本时常附带 RTF/public.* 等额外类型）。
 */
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

/**
 * 检测剪贴板内容类型。
 *
 * 分类流程（v8）：
 *
 *   Step 1 — Web Clipboard API：
 *     - image/*          → raw-image，立即返回
 *
 *   Step 2 — Windows CF_HDROP 探测：
 *     - Rust read_clipboard_file_paths() 提取文件路径列表
 *     - 全部为图片扩展名 → explorer-image-files；否则 → explorer-files
 *
 *   Step 2b — macOS Finder 文件 URL 探测：
 *     - Rust read_clipboard_file_paths_macos() 读取 public.file-url
 *     - 全部为图片扩展名 → finder-image-files；否则 → finder-files
 *
 *   Step 3 — Web API 文本 / 富对象处理：
 *     - macOS：全是 text-like 类型 → plain-text；否则 → rich-object
 *     - Windows/Linux：有非 text/plain & text/html 类型 → rich-object；否则 → plain-text
 *
 *   Step 4 — Tauri fallback（Web API 完全不可用时）：
 *     - preferImage=true  → readImage() 先，readText() 后
 *     - preferImage=false → readText() 先，readImage() 后
 *
 * 设计原则：
 *   - raw-image（截图位图）与 finder/explorer-image-files（文件路径）严格分离
 *   - macOS 文本不因附带 RTF/public.* 而误判为 rich-object
 *   - Windows 现有分支顺序和 kind 名称不变
 */
async function detectClipboardPayload(preferImage = false): Promise<ClipboardPayload> {
  // Step 1: Web Clipboard API — 仅用于快速识别截图图片位图（image/*）
  // WebView2 对 Explorer 文件对象可能返回空 items 或 text/plain（仅文件名），
  // 因此不能仅靠 Web API 来判断是否有 CF_HDROP，必须在 Step 2 独立探测。
  let webItems: ClipboardItem[] = [];
  try {
    webItems = await navigator.clipboard.read();
    debugTerm('clipboard:web_items', {
      itemCount: webItems.length,
      itemTypes: webItems.map((i) => [...i.types]),
      preferImage,
    });
    for (const item of webItems) {
      // 截图工具（微信截图、系统截图等）写入 image/* MIME type → raw-image，立即返回
      // 截图位图优先级最高，不需要再检查 CF_HDROP
      if (item.types.some((t) => t.startsWith('image/'))) {
        debugTerm('clipboard:classified', { kind: 'raw-image', source: 'web-api-image' });
        return { kind: 'raw-image' };
      }
    }
  } catch (err) {
    debugTerm('clipboard:navigator_read_failed', { error: String(err) });
  }

  // Step 2: Windows CF_HDROP 探测（在 Web API 文本处理之前）
  //
  // 为什么放在 Web API text/plain 处理之前：
  //   WebView2 复制 Explorer 文件时，navigator.clipboard.read() 可能返回：
  //     a) 空 items 数组（CF_HDROP 不暴露为 Web 格式）
  //     b) text/plain（仅文件名，无完整路径）
  //   两种情况都无法正确识别 Explorer 文件对象，必须直接读 Win32 CF_HDROP。
  //   CF_HDROP invoke 在没有文件对象时极快（IsClipboardFormatAvailable 立即返回），
  //   不会对纯文本粘贴造成明显延迟。
  if (_isWindows) {
    try {
      const paths = await invoke<string[]>('read_clipboard_file_paths');
      if (paths && paths.length > 0) {
        // 全部是图片扩展名 → explorer-image-files；否则 → explorer-files
        const allImages = paths.every(isImageFilePath);
        return allImages
          ? { kind: 'explorer-image-files', paths }
          : { kind: 'explorer-files', paths };
      }
    } catch { /* 没有 CF_HDROP（非文件对象），继续 */ }
  }

  // Step 2b: macOS Finder 文件 URL 探测（Windows CF_HDROP 对等）
  // WKWebView 通常不暴露 public.file-url，必须通过 Rust 后端原生读取
  if (_isMacOS) {
    try {
      const paths = await invoke<string[]>('read_clipboard_file_paths_macos');
      if (paths && paths.length > 0) {
        const allImages = paths.every(isImageFilePath);
        return allImages
          ? { kind: 'finder-image-files', paths }
          : { kind: 'finder-files', paths };
      }
    } catch { /* 没有 file URL（非文件对象），继续 */ }
  }

  // Step 3: 处理 Web API 的文本 / 富对象结果（Explorer/Finder 文件已在 Step 2 处理完）
  for (const item of webItems) {
    if (_isMacOS) {
      // mac：text-like 类型（含 RTF / public.utf8-plain-text 等）优先走纯文本
      // 避免"复制文本时附带 RTF 类型"被误判为 rich-object
      if (isMacTextLikeClipboardItem(item)) {
        if (item.types.includes('text/plain')) {
          try {
            const blob = await item.getType('text/plain');
            const text = await blob.text();
            if (text.trim()) return { kind: 'plain-text', text };
          } catch { /* ignore */ }
        }
        continue; // text-like 但无有效 text/plain 内容，继续下一个 item
      }
      // 非 text-like → 先用 Tauri readImage() 二次探测 NSPasteboard。
      // macOS WKWebView 可能不暴露 image/* MIME，但 NSPasteboard 中
      // 实际有截图位图（如微信截图）。不检测的话会被误分为 rich-object，
      // 导致发 Ctrl+V 而非 Alt+V，Codex 图片粘贴失败。
      if (preferImage) {
        try {
          const image = await readImage();
          await image.size();
          debugTerm('clipboard:classified', { kind: 'raw-image', source: 'mac-tauri-readImage-fallback' });
          return { kind: 'raw-image' };
        } catch (err) {
          debugTerm('clipboard:readImage_failed', { error: String(err), context: 'mac-rich-object-fallback' });
        }
      }
      debugTerm('clipboard:classified', { kind: 'rich-object', source: 'mac-non-text-like' });
      return { kind: 'rich-object' };
    }

    // Windows / Linux：保持现有逻辑不动
    // 富对象（非 text/plain, 非 text/html）→ rich-object residual fallback
    const hasRich = item.types.some((t) => t !== 'text/plain' && t !== 'text/html');
    if (hasRich) {
      return { kind: 'rich-object' };
    }

    // 纯文本 → plain-text，立即返回，不探图（避免 1-2s 延迟）
    if (item.types.includes('text/plain')) {
      try {
        const blob = await item.getType('text/plain');
        const text = await blob.text();
        if (text.trim()) return { kind: 'plain-text', text };
      } catch { /* ignore */ }
    }
  }

  // Step 4: Tauri fallback（Web API 完全不可用 且 CF_HDROP 无结果时）
  if (preferImage) {
    // AI pane fallback：先尝试图片（截图位图），再尝试文本
    try {
      const image = await readImage();
      await image.size();
      return { kind: 'raw-image' };
    } catch { /* 非图片 */ }
    try {
      const text = await readText();
      if (text && text.trim()) return { kind: 'plain-text', text };
    } catch { /* ignore */ }
  } else {
    // 非 AI pane fallback：先尝试文本，再尝试图片
    // 防止 OS 缓存旧图片（如微信截图后又复制了文字）被 readImage() 误判
    try {
      const text = await readText();
      if (text && text.trim()) return { kind: 'plain-text', text };
    } catch { /* ignore */ }
    try {
      const image = await readImage();
      await image.size();
      return { kind: 'raw-image' };
    } catch { /* 非图片 */ }
  }

  return { kind: 'empty-or-unknown' };
}

/** 读取系统剪贴板并写入终端 PTY。
 *
 * AI pane（Claude / Codex / Gemini CLI）六条独立路径：
 *   plain-text             → sendAiTextPaste                — bracketed-paste，无延迟
 *   raw-image              → sendAiScreenshotImagePaste     — 截图位图，Windows Alt+V
 *   explorer-image-files   → sendAiExplorerImageFilesPaste  — 文件→位图写入剪贴板→Alt+V
 *   finder-image-files     → sendAiExplorerImageFilesPaste  — 同上（macOS）
 *   explorer-files         → sendAiExplorerFilesPaste       — Explorer 普通文件路径注入
 *   finder-files           → sendAiExplorerFilesPaste       — Finder 普通文件路径注入
 *   rich-object            → Ctrl+V                        — 无法识别的富对象，residual fallback
 *
 *   三种"图片相关"类型严格区分（参见顶部注释）：
 *     raw-image              = 截图工具图片位图 → Alt+V，AI CLI 直接读取剪贴板图片数据
 *     explorer-image-files   = Rust 加载文件→写 CF_DIB/TIFF→Alt+V，AI CLI 读取为图片块
 *     explorer-files         = 文件路径引用（非图片扩展名）→ 路径文本注入，文件引用
 *
 * 非 AI pane：
 *   plain-text  → 直接写文本
 *   raw-image   → readImage() 落盘 temp PNG → 平台原生兜底
 *   其余        → 尝试文本兜底
 */
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const provider = getAiProviderForPty(ptyId);
  const isAiPane = !!provider;

  // 诊断：AI pane 身份判定（文档 §5.2）
  if (TERM_DEBUG) {
    const { projectStates } = useAppStore.getState();
    let paneStatus: string | undefined;
    let aiProvider: string | undefined;
    for (const ps of projectStates.values()) {
      for (const tab of ps.tabs) {
        const pane = findPaneByPty(tab.splitLayout, ptyId);
        if (pane) { paneStatus = pane.status; aiProvider = pane.aiProvider; break; }
      }
      if (paneStatus) break;
    }
    debugTerm('paste:ai_identity', {
      ptyId, paneStatus, aiProvider, provider, isAiPane,
    });
  }

  const clipboard = await detectClipboardPayload(isAiPane);

  if (isAiPane) {
    let route = 'noop';
    if (clipboard.kind === 'raw-image') {
      route = 'ai-raw-image';
      // 截图工具图片位图 → provider-aware 快捷键
      // Windows: Alt+V；macOS+codex: Alt+V；macOS+claude: Ctrl+V
      await sendAiScreenshotImagePaste(ptyId, provider);
    } else if (clipboard.kind === 'plain-text' && clipboard.text) {
      route = 'ai-text';
      // 纯文本 → xterm 原生 paste 管道，触发 bracketed-paste 块识别，无延迟
      sendAiTextPaste(ptyId, clipboard.text);
    } else if (
      (clipboard.kind === 'explorer-image-files' || clipboard.kind === 'finder-image-files') &&
      clipboard.paths
    ) {
      route = 'ai-image-files';
      // Explorer / Finder 图片文件：加载为剪贴板位图 → Alt+V，AI CLI 读取为图片块
      // load_image_to_clipboard 将文件写入 CF_DIB/TIFF，再触发与截图相同的粘贴路径
      await sendAiExplorerImageFilesPaste(ptyId, clipboard.paths, provider);
    } else if (
      (clipboard.kind === 'explorer-files' || clipboard.kind === 'finder-files') &&
      clipboard.paths
    ) {
      route = 'ai-files';
      // Explorer / Finder 普通文件路径：路径文本注入，Claude Code 展示文件引用
      // 注意：文件路径引用 ≠ 图片位图，不能走 Alt+V
      sendAiExplorerFilesPaste(ptyId, clipboard.paths);
    } else if (clipboard.kind === 'rich-object') {
      route = 'ai-rich-object-ctrl-v';
      // 无法识别的富对象 residual fallback：Ctrl+V 让 AI CLI 自行决策
      await enqueuePtyWrite(ptyId, '\x16');
    } else {
      // empty-or-unknown：若有附带文本则走文本路径，否则不操作
      if (clipboard.text) {
        route = 'ai-text-fallback';
        sendAiTextPaste(ptyId, clipboard.text);
      }
    }
    debugTerm('paste:route', { ptyId, provider, clipboardKind: clipboard.kind, route });
    return;
  }

  // 非 AI pane
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
      } catch { /* 继续 */ }
    }
    if (_isWindows) {
      try {
        const path: string = await invoke('read_clipboard_image');
        await enqueuePtyWrite(ptyId, path);
        return;
      } catch { /* 继续 */ }
    }
  }

  // rich-object / unknown / 图片落盘全失败：先尝试文本，最后 Alt+V 保险
  if (clipboard.text) {
    await enqueuePtyWrite(ptyId, clipboard.text);
    return;
  }
  await enqueuePtyWrite(ptyId, '\x1bv');
}
