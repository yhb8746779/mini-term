import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../store';
import { getDraggingTabId } from '../utils/dragState';
import { getOrCreateTerminal, getCachedTerminal } from '../utils/terminalCache';
import '@xterm/xterm/css/xterm.css';

type DropZone = 'top' | 'bottom' | 'left' | 'right';
type DragKind = 'file' | 'tab';

function getDropZone(rect: DOMRect, clientX: number, clientY: number): DropZone {
  const x = (clientX - rect.left) / rect.width;
  const y = (clientY - rect.top) / rect.height;
  const aboveMain = y < x;
  const aboveAnti = y < 1 - x;
  if (aboveMain && aboveAnti) return 'top';
  if (!aboveMain && !aboveAnti) return 'bottom';
  if (!aboveMain && aboveAnti) return 'left';
  return 'right';
}

const dropZoneOverlay: Record<DropZone, React.CSSProperties> = {
  top: { top: 0, left: 0, right: 0, height: '50%' },
  bottom: { bottom: 0, left: 0, right: 0, height: '50%' },
  left: { top: 0, left: 0, bottom: 0, width: '50%' },
  right: { top: 0, right: 0, bottom: 0, width: '50%' },
};

interface Props {
  ptyId: number;
  paneId?: string;
  onTabDrop?: (sourceTabId: string, targetPaneId: string, direction: 'horizontal' | 'vertical', position: 'before' | 'after') => void;
}

export function TerminalInstance({ ptyId, paneId, onTabDrop }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [dragKind, setDragKind] = useState<DragKind | null>(null);
  const [tabDropZone, setTabDropZone] = useState<DropZone | null>(null);
  const terminalFontSize = useAppStore((s) => s.config.terminalFontSize);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const { term, fitAddon, wrapper } = getOrCreateTerminal(ptyId);

    container.appendChild(wrapper);

    requestAnimationFrame(() => {
      if (container.clientWidth > 0 && container.clientHeight > 0) {
        fitAddon.fit();
        invoke('resize_pty', { ptyId, cols: term.cols, rows: term.rows });
        term.refresh(0, term.rows - 1);
      }
    });

    let rafId: number;
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(rafId);
      rafId = requestAnimationFrame(() => {
        if (container.clientWidth > 0 && container.clientHeight > 0) {
          fitAddon.fit();
        }
      });
    });
    observer.observe(container);

    const visibilityObserver = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        requestAnimationFrame(() => fitAddon.fit());
      }
    });
    visibilityObserver.observe(container);

    return () => {
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
    }
  }, [terminalFontSize, ptyId]);

  const isTabDrag = (e: React.DragEvent<HTMLDivElement>) => e.dataTransfer.types.includes('application/tab-id');
  const clearDragState = () => {
    setDragKind(null);
    setTabDropZone(null);
  };
  const handleDragMove = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    if (isTabDrag(e)) {
      const rect = e.currentTarget.getBoundingClientRect();
      setDragKind('tab');
      setTabDropZone(getDropZone(rect, e.clientX, e.clientY));
      return;
    }
    setDragKind('file');
    setTabDropZone(null);
  };
  const handleDrop = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    const currentDragKind = dragKind ?? (isTabDrag(e) ? 'tab' : 'file');
    clearDragState();
    if (currentDragKind === 'tab' && paneId && onTabDrop) {
      const tabId = getDraggingTabId();
      if (tabId) {
        const rect = e.currentTarget.getBoundingClientRect();
        const zone = getDropZone(rect, e.clientX, e.clientY);
        const direction: 'horizontal' | 'vertical' =
          zone === 'left' || zone === 'right' ? 'horizontal' : 'vertical';
        const position: 'before' | 'after' =
          zone === 'left' || zone === 'top' ? 'before' : 'after';
        onTabDrop(tabId, paneId, direction, position);
        return;
      }
    }
    const filePath = e.dataTransfer.getData('text/plain').trim();
    if (filePath) {
      const cached = getCachedTerminal(ptyId);
      invoke('write_pty', { ptyId, data: filePath });
      cached?.term.focus();
    }
  };

  return (
    <div className="w-full h-full flex flex-col">
      {/* Terminal content */}
      <div
        className="flex-1 relative bg-[#100f0d]"
        onDragEnterCapture={handleDragMove}
        onDragOverCapture={(e) => {
          handleDragMove(e);
          e.dataTransfer.dropEffect = isTabDrag(e) ? 'move' : 'copy';
        }}
        onDragLeaveCapture={(e) => {
          const nextTarget = e.relatedTarget as Node | null;
          if (!nextTarget || !e.currentTarget.contains(nextTarget)) {
            clearDragState();
          }
        }}
        onDropCapture={handleDrop}
      >
        <div ref={containerRef} className="absolute top-1.5 bottom-0 left-2.5 right-0 cursor-none" />

        {dragKind === 'file' && (
          <div
            className="absolute inset-1 z-10 flex items-center justify-center pointer-events-none rounded-[var(--radius-md)]"
            style={{ background: 'rgba(200, 128, 90, 0.06)', border: '2px dashed var(--accent)' }}
          >
            <span className="text-[var(--accent)] text-xs px-3 py-1.5 rounded-[var(--radius-md)]"
              style={{ background: 'var(--bg-overlay)' }}>
              释放以插入路径
            </span>
          </div>
        )}

        {tabDropZone && (
          <div
            className="absolute z-10 pointer-events-none"
            style={{
              ...dropZoneOverlay[tabDropZone],
              background: 'rgba(200, 128, 90, 0.12)',
              borderRadius: '4px',
            }}
          />
        )}
      </div>
    </div>
  );
}
