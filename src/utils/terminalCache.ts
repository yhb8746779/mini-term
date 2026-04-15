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

// ── 平台级 AI 粘贴快捷键 ──────────────────────────────────────────────────────

/**
 * AI pane 文本粘贴：通过 xterm.js 原生 paste 管道注入文本。
 *
 * term.paste() 会在 terminal 已开启 bracketed-paste 模式时自动包装
 * \x1b[200~...\x1b[201~，让 Claude/Codex/Gemini CLI 识别为粘贴块，
 * 从而触发 [Pasted text #1 +N lines] 预览和大段粘贴安全提示。
 *
 * 为什么不用 enqueuePtyWrite 直接注入 bracketed-paste 字符串：
 *   直接注入绕过了 xterm 的 paste 管道。对某些 TUI/CLI 而言，
 *   这种方式不能被识别为真正的"粘贴事件"，导致"文本没反应"。
 *
 * 为什么要先 focus：
 *   右键等操作可能使焦点从 xterm 上漂移，term.paste() 在失焦状态下
 *   会静默失败；主动 focus 保证调用时终端处于激活状态。
 *
 * 图片粘贴由 sendAiImagePasteShortcut 单独处理，两者不能混用。
 */
function sendAiTextPaste(ptyId: number, text: string): void {
  const entry = cache.get(ptyId);
  if (!entry) return;
  entry.term.focus();
  entry.term.paste(text);
}

/**
 * AI pane 图片粘贴：Windows → Alt+V（\x1bv）；macOS/Linux → Ctrl+V（\x16）。
 *
 * Alt+V 是 Claude/Codex 在 Windows 上的图片粘贴专用键，
 * 仅在剪贴板确实含图片时发送，文本粘贴禁止使用此快捷键。
 */
async function sendAiImagePasteShortcut(ptyId: number): Promise<void> {
  if (_isWindows) {
    await enqueuePtyWrite(ptyId, '\x1bv');
  } else {
    await enqueuePtyWrite(ptyId, '\x16');
  }
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
 * @param preferImage
 *   true  → AI pane 模式：图片/富对象优先。
 *           navigator.clipboard.read() 分支里：
 *             - 读到纯文本：暂存（deferredText），不立即返回，先走图片检测
 *             - 读到富对象：标记 sawRich，不立即返回，先走 readImage() 检测
 *               （Windows 资源管理器复制的图片/文件会报告 rich 类型，但实际可能含图片数据）
 *             - 图片检测成功 → raw-image；失败 → 依次尝试 deferredText / rich-object / readText()
 *   false → 非 AI pane 模式：文本优先。
 *           navigator.clipboard.read() 读到文本或富对象立即返回，
 *           防止 OS 剪贴板缓存旧图片被 readImage() 误判
 *           （如微信截图后又复制了文字的场景）。
 */
async function detectClipboardPayload(preferImage = false): Promise<ClipboardPayload> {
  // Web API 结果暂存（preferImage 模式下延迟返回，让图片检测优先）
  let deferredText: string | undefined;
  let sawRich = false; // navigator.clipboard.read() 发现了富对象类型（非 image/* / text/*）

  // 优先用 Web Clipboard API 读取类型信息（最完整）
  try {
    const items = await navigator.clipboard.read();
    for (const item of items) {
      // 有图片类型 → 两种模式都立即返回，无需继续
      if (item.types.some((t) => t.startsWith('image/'))) {
        return { kind: 'raw-image' };
      }
      // 有文件/URI/其他富对象类型（排除纯文本 / html）
      const hasRich = item.types.some(
        (t) => t !== 'text/plain' && t !== 'text/html',
      );
      const hasText = item.types.includes('text/plain');
      if (hasRich) {
        if (!preferImage) {
          // 非 AI pane：立即返回，不浪费时间做图片检测
          return { kind: 'rich-object' };
        }
        // AI pane：不立即返回。Windows 资源管理器复制的图片会带 rich 类型，
        // 但 readImage() fallback 仍可能拿到图片数据，需要先试一下。
        sawRich = true;
      }
      if (hasText) {
        try {
          const blob = await item.getType('text/plain');
          const text = await blob.text();
          if (text.trim()) {
            if (!preferImage) {
              // 非 AI pane：立即返回文本
              return { kind: 'plain-text', text };
            }
            // AI pane：暂存文本，先做图片检测，防止 sidecar 文本遮蔽图片
            deferredText = text;
          }
        } catch { /* ignore */ }
      }
    }
  } catch { /* Clipboard API 不可用时继续 fallback */ }

  if (preferImage) {
    // AI pane：先尝试图片（涵盖 Windows Explorer 复制图片 / sawRich / deferredText 三种情况）
    try {
      const image = await readImage();
      await image.size();
      return { kind: 'raw-image' };
    } catch { /* 非图片 */ }

    // 图片检测失败，按优先级依次返回：
    // 1. Web API 暂存的文本（sidecar 文本但无图片）
    if (deferredText) return { kind: 'plain-text', text: deferredText };
    // 2. Web API 发现了富对象但不是图片（如 Explorer 复制的非图片文件）
    if (sawRich) return { kind: 'rich-object' };
    // 3. Tauri readText() 兜底
    try {
      const text = await readText();
      if (text && text.trim()) return { kind: 'plain-text', text };
    } catch { /* ignore */ }
  } else {
    // 非 AI pane：文本优先
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
 * AI pane（Claude / Codex / Gemini CLI）：
 *   plain-text → Ctrl+V 让 CLI 从 OS 剪贴板读取，触发 bracketed-paste 块预览
 *   raw-image  → 平台图片快捷键（Windows → Alt+V；macOS/Linux → Ctrl+V）
 *   其他       → Ctrl+V 兜底
 *
 *   重要：不能对 AI pane 直接调用 enqueuePtyWrite() 写纯文本。
 *   Claude/Codex/Gemini CLI 依赖 OS 剪贴板 + bracketed-paste 来识别粘贴块，
 *   直接写入 PTY 会丢失 paste 语义，导致：
 *     - 无 [Pasted text #1 +N lines] 预览
 *     - 无大段粘贴安全提示
 *     - 多行内容被当成连续键入，仅显示第一行
 *   图片粘贴必须保持 Alt+V（Windows），不可用于文本，否则报 "no image" 错误。
 *
 * 非 AI pane：
 *   plain-text → 直接写文本
 *   raw-image  → readImage() 落盘 temp PNG → macOS/Windows 原生兜底
 *   rich-object / unknown → 尝试文本兜底，再 Alt+V 保险
 */
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const isAiPane = !!getAiProviderForPty(ptyId);
  // AI pane 用图片优先检测，非 AI pane 用文本优先检测
  const clipboard = await detectClipboardPayload(isAiPane);

  if (isAiPane) {
    if (clipboard.kind === 'raw-image') {
      // 图片专用快捷键：Windows → Alt+V；macOS/Linux → Ctrl+V
      // 仅在剪贴板确实含图片时发送 Alt+V，文本粘贴禁止使用
      await sendAiImagePasteShortcut(ptyId);
    } else if (clipboard.kind === 'plain-text' && clipboard.text) {
      // 文本：通过 xterm 原生 paste 管道注入，触发 bracketed-paste 块识别
      sendAiTextPaste(ptyId, clipboard.text);
    } else if (clipboard.kind === 'rich-object') {
      // rich-object：Windows 资源管理器复制的图片/文件等。
      // 走 AI 原生粘贴快捷键（Windows → Alt+V），让 Claude/Codex 自行从剪贴板读取。
      // 禁止降级为文本路径，因为这类内容没有 text 字段，且 AI CLI 能处理富剪贴板数据。
      await sendAiImagePasteShortcut(ptyId);
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
