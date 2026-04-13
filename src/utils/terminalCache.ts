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
import { readText, writeText } from '@tauri-apps/plugin-clipboard-manager';
import { useAppStore } from '../store';
import type { PtyOutputPayload } from '../types';
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

  // 右键：有选中文本 → 复制；无选中 → 粘贴（与 Windows Terminal 行为一致）
  wrapper.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    e.stopPropagation();
    const sel = term.getSelection();
    if (sel) {
      writeText(sel);
      term.clearSelection();
    } else {
      readText().then((text) => {
        if (text) {
          invoke('write_pty', { ptyId, data: text });
          term.focus();
        }
      });
    }
  });

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

/** 复制当前终端选中文本到系统剪贴板。无选中则不操作。返回是否有内容被复制。 */
export async function copyTerminalSelection(ptyId: number): Promise<boolean> {
  const cached = cache.get(ptyId);
  if (!cached) return false;
  const sel = cached.term.getSelection();
  if (!sel) return false;
  await writeText(sel);
  return true;
}

/** 读取系统剪贴板并写入终端 PTY。 */
export async function pasteToTerminal(ptyId: number): Promise<void> {
  const text = await readText();
  if (text) await enqueuePtyWrite(ptyId, text);
}
