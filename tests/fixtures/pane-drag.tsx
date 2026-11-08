import React from 'react';
import { createRoot } from 'react-dom/client';
import { TerminalArea } from '../../src/components/TerminalArea';
import { useAppStore } from '../../src/store';
import type { PaneState, SplitNode } from '../../src/types';
import '../../src/styles.css';

const projectId = 'pane-drag-project';
const panes: PaneState[] = ['A', 'B', 'C', 'D'].map((id, index) => ({
  id, customTitle: id, shellName: 'test-shell', status: 'idle', ptyId: index + 1,
}));
const splitLayout: SplitNode = {
  type: 'split', direction: 'horizontal', sizes: [50, 50],
  children: [
    { type: 'leaf', panes: panes.slice(0, 3), activePaneId: 'A' },
    { type: 'leaf', panes: panes.slice(3), activePaneId: 'D' },
  ],
};
useAppStore.setState({
  activeProjectId: projectId,
  config: {
    ...useAppStore.getState().config,
    terminalDisableWebgl: true,
    projects: [{ id: projectId, name: 'Pane drag test', path: '/test' }],
  },
  projectStates: new Map([[projectId, {
    id: projectId, activeTabId: 'test-tab', layoutHydrated: true,
    tabs: [{ id: 'test-tab', status: 'idle', splitLayout }],
  }]]),
});
(window as any).paneDragTest = {
  layout: () => useAppStore.getState().projectStates.get(projectId)!.tabs[0].splitLayout,
};
createRoot(document.getElementById('root')!).render(
  <TerminalArea projectId={projectId} projectPath="/test" />,
);
