import { useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore, genId, collectPtyIds, saveLayoutToConfig } from '../store';
import { TabBar } from './TabBar';
import { SplitLayout } from './SplitLayout';
import { showContextMenu } from '../utils/contextMenu';
import { disposeTerminal } from '../utils/terminalCache';
import type { TerminalTab, PaneState, SplitNode, ShellConfig } from '../types';

interface Props {
  projectId: string;
  projectPath: string;
}

// 收集 SplitNode 树中所有 pane ID
function collectPaneIds(node: SplitNode): string[] {
  if (node.type === 'leaf') return node.panes.map((p) => p.id);
  return node.children.flatMap(collectPaneIds);
}

function insertSplit(
  node: SplitNode,
  targetPaneId: string,
  direction: 'horizontal' | 'vertical',
  newLeaf: SplitNode
): SplitNode {
  if (node.type === 'leaf') {
    if (node.panes.some((p) => p.id === targetPaneId)) {
      return {
        type: 'split',
        direction,
        children: [node, newLeaf],
        sizes: [50, 50],
      };
    }
    return node;
  }
  return {
    ...node,
    children: node.children.map((c) => insertSplit(c, targetPaneId, direction, newLeaf)),
  };
}

function insertSplitNode(
  node: SplitNode,
  targetPaneId: string,
  direction: 'horizontal' | 'vertical',
  newNode: SplitNode,
  position: 'before' | 'after'
): SplitNode {
  if (node.type === 'leaf') {
    if (node.panes.some((p) => p.id === targetPaneId)) {
      const children = position === 'before' ? [newNode, node] : [node, newNode];
      return { type: 'split', direction, children, sizes: [50, 50] };
    }
    return node;
  }
  return {
    ...node,
    children: node.children.map((c) => insertSplitNode(c, targetPaneId, direction, newNode, position)),
  };
}

export function TerminalArea({ projectId, projectPath }: Props) {
  const config = useAppStore((s) => s.config);
  const projectStates = useAppStore((s) => s.projectStates);
  const addTab = useAppStore((s) => s.addTab);
  const updateTabLayout = useAppStore((s) => s.updateTabLayout);
  const removeTab = useAppStore((s) => s.removeTab);
  const ps = projectStates.get(projectId);
  const activeTab = ps?.tabs.find((t) => t.id === ps.activeTabId);

  const handleCloseTab = useCallback(async (tabId: string) => {
    const tab = ps?.tabs.find(t => t.id === tabId);
    if (tab) {
      const ptyIds = collectPtyIds(tab.splitLayout);
      for (const id of ptyIds) {
        await invoke('kill_pty', { ptyId: id });
        disposeTerminal(id);
      }
    }
    removeTab(projectId, tabId);
    saveLayoutToConfig(projectId);
  }, [ps, projectId, removeTab]);

  const handleNewTab = useCallback(async (selectedShell?: ShellConfig) => {
    const shell = selectedShell
      ?? config.availableShells.find((s) => s.name === config.defaultShell)
      ?? config.availableShells[0];
    if (!shell) return;

    const ptyId = await invoke<number>('create_pty', {
      shell: shell.command,
      args: shell.args ?? [],
      cwd: projectPath,
    });

    const paneId = genId();
    const tabId = genId();

    const tab: TerminalTab = {
      id: tabId,
      status: 'idle',
      splitLayout: {
        type: 'leaf',
        panes: [{
          id: paneId,
          shellName: shell.name,
          status: 'idle',
          ptyId,
        }],
        activePaneId: paneId,
      },
    };

    addTab(projectId, tab);
    saveLayoutToConfig(projectId);
  }, [projectId, projectPath, config, addTab]);

  const handleNewTabClick = useCallback((e: React.MouseEvent) => {
    showContextMenu(
      e.clientX,
      e.clientY,
      config.availableShells.map((shell) => ({
        label: shell.name,
        onClick: () => handleNewTab(shell),
      })),
    );
  }, [config.availableShells, handleNewTab]);

  const handleSplitPane = useCallback(
    async (paneId: string, direction: 'horizontal' | 'vertical') => {
      if (!ps || !activeTab) return;
      const shell = config.availableShells.find((s) => s.name === config.defaultShell)
        ?? config.availableShells[0];
      if (!shell) return;

      const ptyId = await invoke<number>('create_pty', {
        shell: shell.command,
        args: shell.args ?? [],
        cwd: projectPath,
      });

      const newPane: PaneState = {
        id: genId(),
        shellName: shell.name,
        status: 'idle',
        ptyId,
      };

      const newLeaf: SplitNode = {
        type: 'leaf',
        panes: [newPane],
        activePaneId: newPane.id,
      };

      const newLayout = insertSplit(activeTab.splitLayout, paneId, direction, newLeaf);
      updateTabLayout(projectId, activeTab.id, newLayout);
      saveLayoutToConfig(projectId);
    },
    [ps, activeTab, config, projectId, projectPath, updateTabLayout]
  );

  const handleTabDrop = useCallback(
    (sourceTabId: string, targetPaneId: string, direction: 'horizontal' | 'vertical', position: 'before' | 'after') => {
      if (!ps || !activeTab) return;
      if (sourceTabId === activeTab.id) return;
      const sourceTab = ps.tabs.find((t) => t.id === sourceTabId);
      if (!sourceTab) return;

      const newLayout = insertSplitNode(
        activeTab.splitLayout,
        targetPaneId,
        direction,
        sourceTab.splitLayout,
        position
      );
      updateTabLayout(projectId, activeTab.id, newLayout);
      removeTab(projectId, sourceTabId);
      saveLayoutToConfig(projectId);
    },
    [ps, activeTab, projectId, updateTabLayout, removeTab]
  );

  // Called when an entire leaf (pane group) is closed.
  // PTYs are already killed by PaneGroup before this is called.
  // For the root leaf case, we close the whole tab.
  const handleCloseLeaf = useCallback((_leafNode: SplitNode) => {
    const currentPs = useAppStore.getState().projectStates.get(projectId);
    const currentTab = currentPs?.tabs.find(t => t.id === currentPs.activeTabId);
    if (!currentTab) return;

    if (currentTab.splitLayout.type === 'leaf') {
      // Root is a leaf -- close the entire tab (PTYs already killed)
      removeTab(projectId, currentTab.id);
      saveLayoutToConfig(projectId);
    }
    // For split layouts, SplitLayout.handleChildClose handles the tree update
    // via onLayoutChange.
  }, [projectId, removeTab]);

  const handleLayoutChange = useCallback((updatedNode: SplitNode) => {
    const currentPs = useAppStore.getState().projectStates.get(projectId);
    const currentActiveTab = currentPs?.tabs.find((t) => t.id === currentPs.activeTabId);
    if (!currentActiveTab) return;

    // Validate layout structure: if pane ID sets differ, discard stale RAF callback
    const currentIds = collectPaneIds(currentActiveTab.splitLayout).sort().join(',');
    const updatedIds = collectPaneIds(updatedNode).sort().join(',');
    if (currentIds !== updatedIds) return;

    updateTabLayout(projectId, currentActiveTab.id, updatedNode);
    saveLayoutToConfig(projectId);
  }, [projectId, updateTabLayout]);

  // Handler for structural changes: tabs added/removed/switched within a leaf,
  // or children removed from a split. Bypasses pane-ID validation since the
  // set of pane IDs is expected to change.
  const handleUpdateNode = useCallback((updatedNode: SplitNode) => {
    const currentPs = useAppStore.getState().projectStates.get(projectId);
    const currentActiveTab = currentPs?.tabs.find((t) => t.id === currentPs.activeTabId);
    if (!currentActiveTab) return;
    updateTabLayout(projectId, currentActiveTab.id, updatedNode);
    saveLayoutToConfig(projectId);
  }, [projectId, updateTabLayout]);

  return (
    <div className="flex flex-col h-full bg-[var(--bg-terminal)]">
      {/* Hide top-level TabBar when there's only one layout tab —
           the PaneGroup tab bar handles everything */}
      {ps && ps.tabs.length > 1 && (
        <TabBar
          projectId={projectId}
          onNewTab={handleNewTabClick}
          onCloseTab={handleCloseTab}
        />
      )}

      <div className="flex-1 overflow-hidden relative">
        {ps?.tabs.map((tab) => (
          <div
            key={tab.id}
            className="absolute inset-0"
            style={{ display: tab.id === ps.activeTabId ? 'block' : 'none' }}
          >
            <SplitLayout
              node={tab.splitLayout}
              projectPath={projectPath}
              onSplit={handleSplitPane}
              onCloseLeaf={handleCloseLeaf}
              onUpdateNode={handleUpdateNode}
              onTabDrop={handleTabDrop}
              onLayoutChange={handleLayoutChange}
            />
          </div>
        ))}

        {(!ps || ps.tabs.length === 0) && (
          <div className="flex flex-col items-center justify-center h-full gap-3 text-[var(--text-muted)]">
            <div className="text-3xl opacity-20">⌘</div>
            <button
              className="px-5 py-2.5 border border-dashed border-[var(--border-default)] rounded-[var(--radius-md)] text-sm hover:border-[var(--accent)] hover:text-[var(--accent)] transition-all duration-200"
              onClick={handleNewTabClick}
            >
              + New Terminal
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
