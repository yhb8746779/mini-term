import { useState, useCallback, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore, genId } from '../store';
import { TerminalInstance } from './TerminalInstance';
import { StatusDot } from './StatusDot';
import { showContextMenu } from '../utils/contextMenu';
import { showConfirm, showPrompt } from '../utils/prompt';
import { disposeTerminal } from '../utils/terminalCache';
import { createPtySafe } from '../utils/createPty';
import type { SplitNode, PaneState, ShellConfig } from '../types';

import { getDragPayload, setDragPayload } from '../utils/dragState';
import type { MoveTarget, Edge } from '../utils/splitTree';

interface Props {
  tabId: string;
  onMovePane: (paneId: string, target: MoveTarget) => void;
  node: SplitNode & { type: 'leaf' };
  projectPath: string;
  onSplit: (paneId: string, direction: 'horizontal' | 'vertical') => void;
  onClosePane: () => void;
  onUpdateNode: (updated: SplitNode) => void;
}

export function PaneGroup({ node, tabId, onMovePane, projectPath, onSplit, onClosePane, onUpdateNode }: Props) {
  const config = useAppStore((s) => s.config);
  const [headerHover, setHeaderHover] = useState(false);

  const [draggedPane, setDraggedPane] = useState<string | null>(null);
  const [insertion, setInsertion] = useState<{ index: number; x: number } | null>(null);
  const [dropZone, setDropZone] = useState<Edge | 'center' | null>(null);
  const clearIndicators = () => {
    setInsertion(null);
    setDropZone(null);
    setDraggedPane(null);
  };
  useEffect(() => {
    // The source may unmount after a successful move; clear every group's feedback.
    const end = () => { setDragPayload(null); clearIndicators(); };
    window.addEventListener('dragend', end);
    window.addEventListener('drop', end);
    return () => {
      window.removeEventListener('dragend', end);
      window.removeEventListener('drop', end);
    };
  }, []);
  const panePayload = () => {
    const payload = getDragPayload();
    return payload?.type === 'pane' && payload.sourceTabId === tabId ? payload : null;
  };
  const getInsertion = (e: React.DragEvent<HTMLDivElement>) => {
    const bar = e.currentTarget;
    const tabs = Array.from(bar.querySelectorAll<HTMLElement>('[data-pane-tab]'));
    const index = tabs.findIndex(tab => e.clientX < tab.getBoundingClientRect().left + tab.offsetWidth / 2);
    const at = index < 0 ? tabs.length : index;
    const rect = bar.getBoundingClientRect();
    const x = at < tabs.length ? tabs[at].getBoundingClientRect().left
      : tabs[tabs.length - 1]?.getBoundingClientRect().right ?? rect.left;
    return { index: at, x: x - rect.left + bar.scrollLeft };
  };
  const getDropZone = (e: React.DragEvent<HTMLDivElement>): Edge | 'center' => {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = (e.clientX - rect.left) / rect.width;
    const y = (e.clientY - rect.top) / rect.height;
    const nearest = Math.min(x, 1 - x, y, 1 - y);
    if (nearest >= 0.25) return 'center';
    if (nearest === x) return 'left';
    if (nearest === 1 - x) return 'right';
    return nearest === y ? 'top' : 'bottom';
  };
  const leave = (e: React.DragEvent<HTMLDivElement>) => {
    if (!e.relatedTarget || !e.currentTarget.contains(e.relatedTarget as Node)) {
      setInsertion(null);
      setDropZone(null);
    }
  };

  const activePane = node.panes.find((p) => p.id === node.activePaneId) ?? node.panes[0];

  const handleNewTab = useCallback(async (selectedShell?: ShellConfig) => {
    const shell = selectedShell
      ?? config.availableShells.find((s) => s.name === config.defaultShell)
      ?? config.availableShells[0];
    if (!shell) return;

    const ptyId = await createPtySafe({
      shell: shell.command,
      args: shell.args ?? [],
      cwd: projectPath,
      shellName: shell.name,
    });
    if (ptyId === null) return;

    const newPane: PaneState = {
      id: genId(),
      shellName: shell.name,
      status: 'idle',
      ptyId,
    };

    onUpdateNode({
      ...node,
      panes: [...node.panes, newPane],
      activePaneId: newPane.id,
    });
  }, [config, projectPath, node, onUpdateNode]);

  const handleNewTabClick = useCallback((e: React.MouseEvent) => {
    if (config.availableShells.length <= 1) {
      handleNewTab();
      return;
    }
    showContextMenu(
      e.clientX,
      e.clientY,
      config.availableShells.map((shell) => ({
        label: shell.name,
        onClick: () => handleNewTab(shell),
      })),
    );
  }, [config.availableShells, handleNewTab]);

  const handleCloseTab = useCallback(async (paneId: string) => {
    const pane = node.panes.find((p) => p.id === paneId);
    if (!pane) return;

    const label = pane.customTitle || pane.shellName;
    const hasAi = pane.status === 'ai-generating' || pane.status === 'ai-thinking' || pane.status === 'ai-awaiting-input' || pane.status === 'ai-complete';
    const title = hasAi ? '关闭 AI 对话' : '关闭终端';
    const message = hasAi
      ? `终端「${label}」正在运行 AI 对话，关闭后对话将被终止，确定继续吗？`
      : `确定要关闭终端「${label}」吗？`;

    const confirmed = await showConfirm(title, message);
    if (!confirmed) return;

    await invoke('kill_pty', { ptyId: pane.ptyId });
    disposeTerminal(pane.ptyId);

    const remaining = node.panes.filter((p) => p.id !== paneId);
    if (remaining.length === 0) {
      onClosePane();
      return;
    }

    const newActive = node.activePaneId === paneId
      ? (remaining[remaining.length - 1]?.id ?? remaining[0].id)
      : node.activePaneId;

    onUpdateNode({
      ...node,
      panes: remaining,
      activePaneId: newActive,
    });
  }, [node, onClosePane, onUpdateNode]);

  const handleRenameTab = useCallback(async (paneId: string) => {
    const pane = node.panes.find((p) => p.id === paneId);
    if (!pane) return;
    const newTitle = await showPrompt('重命名终端', pane.customTitle || pane.shellName);
    if (newTitle === null) return;
    onUpdateNode({
      ...node,
      panes: node.panes.map((p) =>
        p.id === paneId ? { ...p, customTitle: newTitle.trim() || undefined } : p
      ),
    });
  }, [node, onUpdateNode]);

  const handleSetActive = useCallback((paneId: string) => {
    if (paneId !== node.activePaneId) {
      onUpdateNode({ ...node, activePaneId: paneId });
    }
  }, [node, onUpdateNode]);

  const handleClosePaneGroup = useCallback(async () => {
    const aiCount = node.panes.filter(
      (p) => p.status === 'ai-generating' || p.status === 'ai-thinking' || p.status === 'ai-awaiting-input' || p.status === 'ai-complete'
    ).length;
    const title = aiCount > 0 ? '关闭 AI 对话' : '关闭终端';
    const message = aiCount > 0
      ? `该区域内有 ${aiCount} 个终端正在运行 AI 对话，关闭后对话将被终止，确定继续吗？`
      : '确定要关闭该区域内所有终端吗？';

    const confirmed = await showConfirm(title, message);
    if (!confirmed) return;

    for (const pane of node.panes) {
      await invoke('kill_pty', { ptyId: pane.ptyId });
      disposeTerminal(pane.ptyId);
    }
    onClosePane();
  }, [node.panes, onClosePane]);

  if (!activePane) return null;

  return (
    <div className="w-full h-full flex flex-col">
      {/* Tab bar */}
      <div
        className="relative flex bg-[var(--bg-elevated)] border-b border-[var(--border-subtle)] text-[11px] overflow-x-auto select-none shrink-0"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        onDragOver={(e) => {
          if (!panePayload()) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'move';
          setDropZone(null);
          setInsertion(getInsertion(e));
        }}
        onDragLeave={leave}
        onDrop={(e) => {
          const payload = panePayload();
          if (!payload) return;
          e.preventDefault();
          const { index } = getInsertion(e);
          onMovePane(payload.paneId, {
            kind: node.panes.some(p => p.id === payload.paneId) ? 'reorder' : 'into',
            leafKey: node.panes[0].id, index,
          });
          clearIndicators();
        }}
        onMouseEnter={() => setHeaderHover(true)}
        onMouseLeave={() => setHeaderHover(false)}
      >
        {node.panes.map((pane) => {
          const isActive = pane.id === activePane.id;
          return (
            <div
              key={pane.id}
              data-pane-tab={pane.id}
              draggable
              style={{ opacity: draggedPane === pane.id ? 0.4 : 1 }}
              onDragStart={(e) => {
                setDragPayload({ type: 'pane', paneId: pane.id, sourceTabId: tabId });
                e.dataTransfer.effectAllowed = 'move';
                e.dataTransfer.setData('application/x-mini-term-pane', pane.id);
                setDraggedPane(pane.id);
              }}
              onDragEnd={() => { setDragPayload(null); clearIndicators(); }}
              className={`flex items-center gap-1.5 px-3 py-[3px] cursor-pointer whitespace-nowrap transition-all duration-100 relative ${
                isActive
                  ? 'bg-[var(--bg-terminal)] text-[var(--text-primary)]'
                  : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--border-subtle)]'
              }`}
              onClick={() => handleSetActive(pane.id)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                showContextMenu(e.clientX, e.clientY, [
                  { label: '重命名', onClick: () => handleRenameTab(pane.id) },
                ]);
              }}
            >
              {isActive && (
                <span className="absolute bottom-0 left-2 right-2 h-[2px] rounded-full bg-[var(--accent)]" />
              )}
              <StatusDot status={pane.status} provider={pane.aiProvider} />
              <span className="font-medium">{pane.customTitle || pane.shellName}</span>
              <span
                className="ml-0.5 text-[var(--text-muted)] hover:text-[var(--color-error)] text-[12px] transition-colors"
                onClick={(e) => {
                  e.stopPropagation();
                  handleCloseTab(pane.id);
                }}
              >
                ✕
              </span>
            </div>
          );
        })}

        {insertion && (
          <div className="absolute top-0 bottom-0 w-[2px] bg-[var(--accent)] pointer-events-none z-20"
            style={{ left: insertion.x }} />
        )}
        {/* "+" button */}
        <div
          className="px-2 py-[3px] text-[var(--text-muted)] cursor-pointer hover:text-[var(--accent)] transition-colors text-[12px]"
          onClick={handleNewTabClick}
        >
          +
        </div>

        {/* Right-aligned split/close controls (on hover) */}
        <div
          className="ml-auto flex items-center gap-0.5 px-2 text-[12px]"
        >
          <div
            className="flex items-center gap-0.5 transition-opacity duration-150"
            style={{ opacity: headerHover ? 1 : 0 }}
          >
            <span
              className="text-[var(--text-muted)] hover:text-[var(--accent)] cursor-pointer transition-colors px-0.5"
              title="Split right"
              onClick={() => onSplit(activePane.id, 'horizontal')}
            >
              ┃
            </span>
            <span
              className="text-[var(--text-muted)] hover:text-[var(--accent)] cursor-pointer transition-colors px-0.5"
              title="Split down"
              onClick={() => onSplit(activePane.id, 'vertical')}
            >
              ━
            </span>
            <span
              className="text-[var(--text-muted)] hover:text-[var(--color-error)] cursor-pointer transition-colors pl-0.5"
              title="Close pane"
              onClick={handleClosePaneGroup}
            >
              ✕
            </span>
          </div>
        </div>
      </div>

      {/* Active terminal */}
      <div className="flex-1 overflow-hidden relative"
        onDragOver={(e) => {
          if (!panePayload()) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = 'move';
          setInsertion(null);
          setDropZone(getDropZone(e));
        }}
        onDragLeave={leave}
        onDrop={(e) => {
          const payload = panePayload();
          if (!payload) return;
          e.preventDefault();
          const zone = getDropZone(e);
          onMovePane(payload.paneId, zone === 'center'
            ? { kind: 'into', leafKey: node.panes[0].id }
            : { kind: 'edge', leafKey: node.panes[0].id, edge: zone });
          clearIndicators();
        }}
      >
        {node.panes.map((pane) => (
          <div
            key={pane.ptyId}
            className="absolute inset-0"
            style={{ display: pane.id === activePane.id ? 'block' : 'none' }}
          >
            <TerminalInstance
              ptyId={pane.ptyId}
            />
          </div>
        ))}
        {dropZone && (
          <div className="absolute z-20 pointer-events-none flex items-center justify-center"
            style={{
              background: 'var(--accent-subtle)', border: '2px solid var(--accent)',
              left: dropZone === 'right' ? '50%' : dropZone === 'center' ? '25%' : 0,
              right: dropZone === 'left' ? '50%' : dropZone === 'center' ? '25%' : 0,
              top: dropZone === 'bottom' ? '50%' : dropZone === 'center' ? '25%' : 0,
              bottom: dropZone === 'top' ? '50%' : dropZone === 'center' ? '25%' : 0,
            }}>
            <span className="text-xs text-[var(--accent)]">
              {dropZone === 'center' ? '移入此组' : '释放以分屏'}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
