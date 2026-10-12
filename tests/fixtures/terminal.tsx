import React from 'react';
import { createRoot } from 'react-dom/client';
import { TerminalInstance } from '../../src/components/TerminalInstance';
import { getCachedTerminal, pasteToTerminal, getAnySelectedText } from '../../src/utils/terminalCache';
import { useAppStore } from '../../src/store';
import { SshImageBridgeSettings } from '../../src/components/SettingsModal';
import { SshImageBridgeSelect } from '../../src/components/SshImageBridgeSelect';

const root = createRoot(document.getElementById('root')!);

// Real React component, xterm, clipboard classifier and paste router; only the
// native OS boundary is mocked by the test. Never connect to a user's PTY.
window.terminalTest = {
  config: () => useAppStore.getState().config,
  setBridges: (bridges: any[], activeId?: string) => useAppStore.setState({
    config: { ...useAppStore.getState().config, sshImageBridges: bridges, activeSshImageBridgeId: activeId },
  }),
  showBridgeSettings: () => root.render(<><SshImageBridgeSelect compact /><SshImageBridgeSettings /></>),
  paste: () => pasteToTerminal(1),
  term: () => getCachedTerminal(1)!.term,
  selected: () => getAnySelectedText(1),
  provider: (provider: string) => useAppStore.setState({
    projectStates: new Map([['test', { tabs: [{ splitLayout: {
      type: 'leaf', panes: [{ ptyId: 1, status: 'ai-awaiting-input', aiProvider: provider }],
    } }] }]]) as any,
  }),
};
window.terminalTest.provider('codex');
useAppStore.setState({ config: { ...useAppStore.getState().config, terminalDisableWebgl: true } });
root.render(<TerminalInstance ptyId={1} />);
