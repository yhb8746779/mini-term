// === 配置持久化 ===

export interface AppConfig {
  projects: ProjectConfig[];
  defaultShell: string;
  availableShells: ShellConfig[];
}

export interface ProjectConfig {
  id: string;
  name: string;
  path: string;
}

export interface ShellConfig {
  name: string;
  command: string;
  args?: string[];
}

// === 运行时状态 ===

export type PaneStatus = 'idle' | 'running' | 'ai-working' | 'error';
export type TabStatus = PaneStatus;

export interface ProjectState {
  id: string;
  tabs: TerminalTab[];
  activeTabId: string;
}

export interface TerminalTab {
  id: string;
  customTitle?: string;
  splitLayout: SplitNode;
  status: TabStatus;
}

export type SplitNode =
  | { type: 'leaf'; pane: PaneState }
  | { type: 'split'; direction: 'horizontal' | 'vertical'; children: SplitNode[]; sizes: number[] };

export interface PaneState {
  id: string;
  shellName: string;
  status: PaneStatus;
  ptyId: number;
}

// === AI 会话 ===

export interface AISession {
  id: string;
  sessionType: 'claude' | 'codex';
  projectPath: string;
  startTime: string;
  messageCount: number;
}

// === 文件树 ===

export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  children?: FileEntry[];
}

// === Tauri 事件 payload ===

export interface PtyOutputPayload {
  ptyId: number;
  data: string;
}

export interface PtyExitPayload {
  ptyId: number;
  exitCode: number;
}

export interface PtyStatusChangePayload {
  ptyId: number;
  status: PaneStatus;
}

export interface FsChangePayload {
  projectPath: string;
  path: string;
  kind: string;
}
