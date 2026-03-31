// === 配置持久化 ===

export interface ProjectGroup {
  id: string;
  name: string;
  collapsed: boolean;
  projectIds: string[];
}

export interface AppConfig {
  projects: ProjectConfig[];
  projectGroups?: ProjectGroup[];
  projectOrdering?: string[];
  defaultShell: string;
  availableShells: ShellConfig[];
  uiFontSize: number;
  terminalFontSize: number;
  layoutSizes?: number[];
}

export interface ProjectConfig {
  id: string;
  name: string;
  path: string;
  savedLayout?: SavedProjectLayout;
}

export interface ShellConfig {
  name: string;
  command: string;
  args?: string[];
}

// === 布局持久化 ===

export interface SavedPane {
  shellName: string;
}

export type SavedSplitNode =
  | { type: 'leaf'; pane: SavedPane }
  | { type: 'split'; direction: 'horizontal' | 'vertical'; children: SavedSplitNode[]; sizes: number[] };

export interface SavedTab {
  customTitle?: string;
  splitLayout: SavedSplitNode;
}

export interface SavedProjectLayout {
  tabs: SavedTab[];
  activeTabIndex: number;
}

// === 运行时状态 ===

export type PaneStatus = 'idle' | 'ai-idle' | 'ai-working' | 'error';

export interface ProjectState {
  id: string;
  tabs: TerminalTab[];
  activeTabId: string;
}

export interface TerminalTab {
  id: string;
  customTitle?: string;
  splitLayout: SplitNode;
  status: PaneStatus;
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

export interface AiSession {
  id: string;
  sessionType: 'claude' | 'codex';
  title: string;
  timestamp: string; // ISO 8601
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

// === Git 状态 ===

export type GitStatusType = 'modified' | 'added' | 'deleted' | 'renamed' | 'untracked' | 'conflicted';

export interface GitFileStatus {
  path: string;
  oldPath?: string;
  status: GitStatusType;
  statusLabel: string; // "M", "A", "D", "R", "?", "C"
}

export interface DiffHunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
}

export interface DiffLine {
  kind: 'add' | 'delete' | 'context';
  content: string;
  oldLineno?: number;
  newLineno?: number;
}

export interface GitDiffResult {
  oldContent: string;
  newContent: string;
  hunks: DiffHunk[];
  isBinary: boolean;
  tooLarge: boolean;
}

// === 文件查看 ===

export interface FileContentResult {
  content: string;
  isBinary: boolean;
  tooLarge: boolean;
}
