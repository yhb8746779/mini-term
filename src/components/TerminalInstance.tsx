import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../store';
import { getOrCreateTerminal, getCachedTerminal, getTerminalTheme, DARK_TERMINAL_THEME, writePtyInput, copyTextToClipboard, pasteToTerminal, getAnySelectedText, _isWindows } from '../utils/terminalCache';
import { getResolvedTheme } from '../utils/themeManager';
import { showContextMenu } from '../utils/contextMenu';
import '@xterm/xterm/css/xterm.css';

interface Props {
  ptyId: number;
}

const INITIAL_PTY_RESIZE_DELAY = 320;
const INITIAL_PTY_RESIZE_MIN_COLS = 40;

export function TerminalInstance({ ptyId }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [fileDrag, setFileDrag] = useState(false);
  const terminalFontSize = useAppStore((s) => s.config.terminalFontSize);
  const terminalFollowTheme = useAppStore((s) => s.config.terminalFollowTheme);

  // 终端不跟随主题且处于浅色模式时，面板背景强制深色
  const forceDarkBg = !terminalFollowTheme && getResolvedTheme() === 'light';
  const panelBg = forceDarkBg ? DARK_TERMINAL_THEME.background : undefined;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const { term, fitAddon, wrapper } = getOrCreateTerminal(ptyId);
    const mountAt = performance.now();

    const syncTerminalSize = (forcePtyResize = false) => {
      if (container.clientWidth <= 0 || container.clientHeight <= 0) return;
      fitAddon.fit();
      term.refresh(0, Math.max(term.rows - 1, 0));

      const startupWindow = performance.now() - mountAt < INITIAL_PTY_RESIZE_DELAY;
      const shouldDelayPtyResize = !forcePtyResize && startupWindow && term.cols < INITIAL_PTY_RESIZE_MIN_COLS;
      if (!shouldDelayPtyResize) {
        invoke('resize_pty', { ptyId, cols: term.cols, rows: term.rows });
      }
    };

    container.appendChild(wrapper);

    // 双层 rAF：让 Allotment 完成布局计算后再测量容器尺寸，避免在过渡尺寸时 fit() 得到错误的 cols
    requestAnimationFrame(() => {
      requestAnimationFrame(() => syncTerminalSize());
    });

    // 启动期兜底：避免 PTY 在布局尚未稳定时先被缩到很窄，导致 PowerShell banner/prompt 被硬换行。
    const fallbackId = window.setTimeout(() => syncTerminalSize(true), INITIAL_PTY_RESIZE_DELAY);

    let rafId: number;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(() => syncTerminalSize());
    });
    observer.observe(container);

    const visibilityObserver = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        requestAnimationFrame(() => syncTerminalSize(true));
      }
    });
    visibilityObserver.observe(container);

    return () => {
      window.clearTimeout(fallbackId);
      cancelAnimationFrame(rafId);
      observer.disconnect();
      visibilityObserver.disconnect();
      wrapper.remove();
    };
  }, [ptyId]);

  useEffect(() => {
    const cached = getCachedTerminal(ptyId);
    if (cached && terminalFontSize) {
      cached.term.options.fontSize = terminalFontSize;
      cached.fitAddon.fit();
      cached.term.refresh(0, Math.max(cached.term.rows - 1, 0));
      invoke('resize_pty', { ptyId, cols: cached.term.cols, rows: cached.term.rows });
    }
  }, [terminalFontSize, ptyId]);

  useEffect(() => {
    const handler = () => {
      const cached = getCachedTerminal(ptyId);
      if (cached) {
        const { config } = useAppStore.getState();
        cached.term.options.theme = getTerminalTheme(config.terminalFollowTheme ?? true);
      }
    };
    window.addEventListener('theme-changed', handler);
    return () => window.removeEventListener('theme-changed', handler);
  }, [ptyId]);

  const handleDragOver = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    setFileDrag(true);
  };
  const handleDragLeave = (e: React.DragEvent<HTMLDivElement>) => {
    const nextTarget = e.relatedTarget as Node | null;
    if (!nextTarget || !e.currentTarget.contains(nextTarget)) {
      setFileDrag(false);
    }
  };
  const handleDrop = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setFileDrag(false);
    const filePath = e.dataTransfer.getData('text/plain').trim();
    if (filePath) {
      void writePtyInput(ptyId, filePath);
      getCachedTerminal(ptyId)?.term.focus();
    }
  };

  const handleContextMenu = (e: React.MouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    const selectedText = getAnySelectedText(ptyId);

    // Windows：与 Windows Terminal 一致，右键直接复制或粘贴，不弹菜单
    if (_isWindows) {
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

    // macOS / Linux：弹自定义菜单
    // 菜单关闭后统一回焦终端（复制/粘贴任意操作后都生效）
    const refocusTerminal = () => {
      requestAnimationFrame(() => {
        getCachedTerminal(ptyId)?.term.focus();
      });
    };
    showContextMenu(e.clientX, e.clientY, [
      {
        label: '复制',
        disabled: !selectedText,
        onClick: () => { void copyTextToClipboard(selectedText); },
      },
      {
        label: '粘贴',
        onClick: () => { void pasteToTerminal(ptyId); },
      },
    ], refocusTerminal);
  };

  return (
    <div className="w-full h-full flex flex-col">
      <div
        className="flex-1 relative bg-[var(--bg-terminal)]"
        style={panelBg ? { backgroundColor: panelBg } : undefined}
        onDragOverCapture={handleDragOver}
        onDragLeaveCapture={handleDragLeave}
        onDropCapture={handleDrop}
        onContextMenu={handleContextMenu}
      >
        <div ref={containerRef} className="absolute top-1.5 bottom-0 left-2.5 right-0 cursor-none" />

        {fileDrag && (
          <div
            className="absolute inset-1 z-10 flex items-center justify-center pointer-events-none rounded-[var(--radius-md)]"
            style={{ background: 'var(--accent-subtle)', border: '2px dashed var(--accent)' }}
          >
            <span className="text-[var(--accent)] text-xs px-3 py-1.5 rounded-[var(--radius-md)]"
              style={{ background: 'var(--bg-overlay)' }}>
              释放以插入路径
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
