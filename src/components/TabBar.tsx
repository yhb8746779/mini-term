import { useAppStore } from '../store';
import { StatusDot } from './StatusDot';
import { setDraggingTabId } from '../utils/dragState';
import type { TerminalTab } from '../types';

function getFirstLeafName(node: TerminalTab['splitLayout']): string {
  if (node.type === 'leaf') return node.panes[0]?.shellName ?? 'Terminal';
  return getFirstLeafName(node.children[0]);
}

function getTabTitle(tab: TerminalTab): string {
  if (tab.customTitle) return tab.customTitle;
  return getFirstLeafName(tab.splitLayout);
}

interface Props {
  projectId: string;
  onNewTab: (e: React.MouseEvent) => void;
  onCloseTab: (tabId: string) => void;
}

export function TabBar({ projectId, onNewTab, onCloseTab }: Props) {
  const projectStates = useAppStore((s) => s.projectStates);
  const setActiveTab = useAppStore((s) => s.setActiveTab);
  const ps = projectStates.get(projectId);
  if (!ps) return null;

  return (
    <div className="flex bg-[var(--bg-elevated)] border-b border-[var(--border-subtle)] text-[11px] overflow-x-auto select-none">
      {ps.tabs.map((tab) => {
        const isActive = tab.id === ps.activeTabId;
        return (
          <div
            key={tab.id}
            className={`flex items-center gap-1.5 px-3 py-[3px] cursor-pointer whitespace-nowrap transition-all duration-100 relative ${
              isActive
                ? 'bg-[var(--bg-terminal)] text-[var(--text-primary)]'
                : 'text-[var(--text-muted)] hover:text-[var(--text-secondary)] hover:bg-[var(--border-subtle)]'
            }`}
            draggable
            onDragStart={(e) => {
              setDraggingTabId(tab.id);
              e.dataTransfer.setData('application/tab-id', tab.id);
              e.dataTransfer.effectAllowed = 'move';
            }}
            onDragEnd={() => {
              setDraggingTabId(null);
            }}
            onClick={() => setActiveTab(projectId, tab.id)}
          >
            {isActive && (
              <span className="absolute bottom-0 left-2 right-2 h-[2px] rounded-full bg-[var(--accent)]" />
            )}
            <StatusDot status={tab.status} />
            <span className="font-medium">{getTabTitle(tab)}</span>
            <span
              className="ml-0.5 text-[var(--text-muted)] hover:text-[var(--color-error)] text-[12px] transition-colors"
              onClick={(e) => {
                e.stopPropagation();
                onCloseTab(tab.id);
              }}
            >
              ✕
            </span>
          </div>
        );
      })}
      <div
        className="px-2 py-[3px] text-[var(--text-muted)] cursor-pointer hover:text-[var(--accent)] transition-colors text-[12px]"
        onClick={onNewTab}
      >
        +
      </div>
    </div>
  );
}
