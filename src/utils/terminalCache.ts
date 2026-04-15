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

export function getOrCreateTerminal(ptyId: number): CachedTerminal {
  const existing = cache.get(ptyId);
  if (existing) return existing;

  // 创建 wrapper 容器，xterm.js 会在其中渲染
  const wrapper = document.createElement('div');
  wrapper.style.width = '100%';
  wrapper.style.height = '100%';

  const term = new Terminal({
    fontSize: useAppStore.getState().config.terminalFontSize ?? 14,
    // CJK 备用字体：PingFang SC（macOS）/ Noto Sans Mono CJK SC（Linux）/ Microsoft YaHei（Windows）
    // 确保中文字符有合适的字形，避免宽度计算与实际渲染不一致导致乱码
    fontFamily: "'JetBrains Mono', 'Cascadia Code', Consolas, 'PingFang SC', 'Hiragino Sans GB', 'Noto Sans Mono CJK SC', 'Microsoft YaHei Mono', monospace",
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
  try {
    const webgl = new WebglAddon();
    webgl.onContextLoss(() => {
      webgl.dispose();
      term.refresh(0, term.rows - 1);
    });
    term.loadAddon(webgl);
  } catch {
    // WebGL 不支持
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

// ── AI pane 三条独立粘贴路径 ──────────────────────────────────────────────────
//
// 剪贴板来源必须区分为三类，不可互换：
//
//   1. plain-text       → sendAiTextPaste
//   2. raw-image        → sendAiScreenshotImagePaste  （截图工具图片位图）
//   3. rich-object      → sendAiNativeHandoff          （Explorer 文件系统对象）
//
// raw-image ≠ rich-object：
//   - raw-image  = 截图工具/图片编辑器直接写入剪贴板的图片位图数据，
//                  Mini-Term 有增强处理路径（Alt+V on Windows）。
//   - rich-object = Explorer 复制的文件/Shell 对象/文件引用列表等，
//                  不是图片位图，不应走图片快捷键，应 native handoff。
//
// 混用两者会导致：Explorer 文件对象走 Alt+V → "no image" 错误；
//              截图图片走文本路径 → 图片内容丢失。

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
 * Windows → Alt+V（\x1bv）：Claude/Codex 图片粘贴专用快捷键。
 * macOS/Linux → Ctrl+V（\x16）。
 *
 * 重要限制：
 *   - 仅在剪贴板内容确实是图片位图时使用（raw-image 分类）
 *   - 禁止对 Explorer 文件对象（rich-object）使用，否则报"no image"错误
 *   - 禁止对纯文本使用
 */
async function sendAiScreenshotImagePaste(ptyId: number): Promise<void> {
  if (_isWindows) {
    await enqueuePtyWrite(ptyId, '\x1bv');
  } else {
    await enqueuePtyWrite(ptyId, '\x16');
  }
}

/**
 * AI pane Explorer/富对象 native handoff 路径。
 *
 * 用于 Windows 资源管理器复制的文件/图片文件/Shell 对象等富剪贴板内容。
 * 这类内容不是图片位图，不能走 sendAiScreenshotImagePaste（Alt+V）。
 *
 * 策略：
 *   1. 优先：通过 Tauri Rust 读取 CF_HDROP 文件路径列表
 *      → 把路径注入 AI pane（Claude Code 收到路径后，自行判断是图片还是文件）
 *      → 图片文件路径 → Claude Code 展示图片块
 *      → 普通文件路径 → Claude Code 展示文件引用
 *   2. 降级：CF_HDROP 读取失败或非 Windows 时发送 Ctrl+V（\x16）
 *      → 将粘贴决策权交给 AI CLI 自身
 *
 * 为什么通过文件路径而非直接发图片：
 *   Explorer 复制的是文件引用（CF_HDROP），不是图片位图（CF_DIB）；
 *   位图数据由截图工具写入，文件引用只有路径。
 *   Claude Code 可以通过路径自行加载图片，效果等同于原生 PowerShell 粘贴。
 */
async function sendAiNativeHandoff(ptyId: number): Promise<void> {
  // 尝试读取 CF_HDROP 文件路径（Windows Explorer 复制文件时使用此格式）
  if (_isWindows) {
    try {
      const paths = await invoke<string[]>('read_clipboard_file_paths');
      if (paths && paths.length > 0) {
        // 把文件路径注入 AI pane：Claude Code 自行判断图片/文件
        // 多个路径用空格分隔（与原生 PowerShell 粘贴行为一致）
        const text = paths.join(' ');
        sendAiTextPaste(ptyId, text);
        return;
      }
    } catch {
      // CF_HDROP 读取失败（非文件对象、权限问题等），降级到 Ctrl+V
    }
  }
  // 非 Windows 或 CF_HDROP 不可用：Ctrl+V，让 AI CLI 从 OS 剪贴板原生读取
  await enqueuePtyWrite(ptyId, '\x16');
}

// ── 剪贴板内容分类器 ────────────────────────────────────────────────────────────

type ClipboardPayloadKind = 'plain-text' | 'raw-image' | 'rich-object' | 'empty-or-unknown';

interface ClipboardPayload {
  kind: ClipboardPayloadKind;
  text?: string;
}

/**
 * 检测剪贴板内容类型。
 *
 * 分类优先级（Web Clipboard API 可用时）：
 *   1. item.types 含 image/*        → raw-image（两种模式相同）
 *   2. item.types 含非 text/* 类型  → rich-object（两种模式相同）
 *      rich-object ≠ raw-image：Explorer 复制的文件/Shell对象是富剪贴板对象，
 *      不是图片位图，AI pane 必须分开处理，不得把 rich-object 路由到图片快捷键。
 *   3. item.types 仅含 text/plain   → plain-text，立即返回
 *      AI pane 文本已明确时不得先跑 readImage()，否则造成 1-2s 可见延迟。
 *
 * Web API 不可用时才走 Tauri fallback：
 *   preferImage=true  (AI pane)  → 先 readImage()，再 readText()
 *   preferImage=false (非AI pane) → 先 readText()，再 readImage()
 */
async function detectClipboardPayload(preferImage = false): Promise<ClipboardPayload> {
  // 优先用 Web Clipboard API（类型信息最权威）
  try {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      // Case 1: 明确图片 → raw-image，两种模式均立即返回
      if (item.types.some((t) => t.startsWith('image/'))) {
        return { kind: 'raw-image' };
      }

      // Case 2: 富对象（文件/Shell/URI 等，排除 text/plain 和 text/html）→ rich-object
      // 注意：rich-object 不等于 raw-image；Explorer 文件对象不是图片位图，
      // AI pane 需要用专属路径处理，不得走图片快捷键（Alt+V）。
      const hasRich = item.types.some(
        (t) => t !== 'text/plain' && t !== 'text/html',
      );
      if (hasRich) {
        return { kind: 'rich-object' };
      }

      // Case 3: 只有纯文本 → plain-text，立即返回
      // AI pane：文本已明确，不得继续跑 readImage()，否则产生可见延迟。
      if (item.types.includes('text/plain')) {
        try {
          const blob = await item.getType('text/plain');
          const text = await blob.text();
          if (text.trim()) return { kind: 'plain-text', text };
        } catch { /* ignore */ }
      }
    }
  } catch { /* Clipboard API 不可用，继续走 Tauri fallback */ }

  // Case 4: Web API 不可用或未返回有效内容，走 Tauri fallback
  if (preferImage) {
    // AI pane fallback：先尝试图片，再尝试文本
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
 * AI pane（Claude / Codex / Gemini CLI）三条独立路径：
 *   plain-text  → sendAiTextPaste     — xterm bracketed-paste，不探图，无延迟
 *   raw-image   → sendAiScreenshotImagePaste — 截图图片位图，Windows Alt+V
 *   rich-object → sendAiNativeHandoff — Explorer 文件/Shell 对象，Ctrl+V native handoff
 *
 *   raw-image 和 rich-object 必须分开：
 *     截图图片位图 → Alt+V 让 Claude/Codex 读取图片
 *     Explorer 文件对象 → Alt+V 报"no image"，必须用 native handoff
 *
 * 非 AI pane：
 *   plain-text  → 直接写文本
 *   raw-image   → readImage() 落盘 temp PNG → 平台原生兜底
 *   rich-object / unknown → 尝试文本兜底
 */
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const isAiPane = !!getAiProviderForPty(ptyId);
  const clipboard = await detectClipboardPayload(isAiPane);

  if (isAiPane) {
    if (clipboard.kind === 'raw-image') {
      // 截图工具图片位图 → Mini-Term 增强图片路径（Windows: Alt+V）
      // 保留用户已有的右键截图粘贴体验
      await sendAiScreenshotImagePaste(ptyId);
    } else if (clipboard.kind === 'plain-text' && clipboard.text) {
      // 纯文本 → xterm 原生 paste 管道，触发 bracketed-paste 块识别
      sendAiTextPaste(ptyId, clipboard.text);
    } else if (clipboard.kind === 'rich-object') {
      // Explorer 文件/Shell 对象 → native handoff（Ctrl+V）
      // 不得走截图图片路径（Alt+V），Explorer 文件对象不是图片位图
      await sendAiNativeHandoff(ptyId);
    } else {
      // empty-or-unknown：若有附带文本则走文本路径，否则不操作
      if (clipboard.text) {
        sendAiTextPaste(ptyId, clipboard.text);
      }
    }
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
