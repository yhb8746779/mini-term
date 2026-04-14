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

const _isMacOS = /Mac OS X|Macintosh/.test(navigator.userAgent);
const _isWindows = /Windows/.test(navigator.userAgent);

/**
 * 第一层：通过 readImage() 直接取像素后落盘成临时 PNG（跨平台主路径）。
 * 返回临时文件路径；任何环节失败返回 null。
 */
/** 轻量探测：剪贴板是否含有图片数据（不落盘，只读尺寸） */
async function clipboardHasImageData(): Promise<boolean> {
  try {
    const image = await readImage();
    await image.size();
    return true;
  } catch {
    return false;
  }
}

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

/**
 * 按 provider + platform 发送对应的"原生图片粘贴"快捷键。
 *
 * macOS:
 *   claude  → Ctrl+V (\x16)   Claude Code 在 mac 上响应 Ctrl+V 触发图片附件
 *   codex   → Alt+V  (\x1bv)  本机已验证可工作的路径
 *   gemini  → Ctrl+V (\x16)
 * Windows / Linux:
 *   claude  → Alt+V  (\x1bv)
 *   codex   → Ctrl+V (\x16)
 *   gemini  → Ctrl+V (\x16)
 */
async function sendProviderImagePasteShortcut(
  ptyId: number,
  provider: AiProvider,
): Promise<void> {
  if (_isMacOS) {
    if (provider === 'codex') {
      await enqueuePtyWrite(ptyId, '\x1bv'); // Alt+V：本机已验证
    } else {
      await enqueuePtyWrite(ptyId, '\x16');  // Ctrl+V：claude / gemini
    }
    return;
  }
  if (_isWindows) {
    if (provider === 'claude') {
      await enqueuePtyWrite(ptyId, '\x1bv'); // Alt+V：Windows claude
    } else {
      await enqueuePtyWrite(ptyId, '\x16');  // Ctrl+V：codex / gemini
    }
    return;
  }
  // Linux / fallback
  await enqueuePtyWrite(ptyId, '\x16');
}

/** 读取系统剪贴板并写入终端 PTY。
 *
 * AI pane（按 provider + platform 分流）：
 *   macOS claude  → Ctrl+V；macOS codex → Alt+V；macOS gemini → Ctrl+V
 *   Windows claude → Alt+V；其余 → Ctrl+V
 * 非 AI pane：
 *   readImage() 落盘 → temp PNG 路径 → macOS/Windows 原生兜底 → 纯文本 → Alt+V
 */
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const hasImage = await clipboardHasImageData();
  const provider = getAiProviderForPty(ptyId);

  // AI pane：按 provider + platform 发送专属图片粘贴快捷键
  if (hasImage && provider) {
    await sendProviderImagePasteShortcut(ptyId, provider);
    return;
  }

  // 非 AI pane：标准落盘路径
  if (hasImage) {
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
      } catch {
        // NSPasteboard 读取失败，继续
      }
    }

    if (_isWindows) {
      try {
        const path: string = await invoke('read_clipboard_image');
        await enqueuePtyWrite(ptyId, path);
        return;
      } catch {
        // CF_DIB/CF_BITMAP 读取失败，继续
      }
    }

    // 图片存在但所有落盘路径均失败，退回 Alt+V
    await enqueuePtyWrite(ptyId, '\x1bv');
    return;
  }

  // 无图片：纯文本
  const text = await readText().catch(() => null);
  if (text) await enqueuePtyWrite(ptyId, text);
}
