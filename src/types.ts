// === 配置持久化 ===

export type ProjectTreeItem = string | ProjectGroup;

export interface ProjectGroup {
  id: string;
  name: string;
  collapsed: boolean;
  children: ProjectTreeItem[];
}

export interface AppConfig {
  projects: ProjectConfig[];
  projectTree?: ProjectTreeItem[];
  // 旧字段仅用于迁移兼容（Rust 端处理后不再出现）
  projectGroups?: { id: string; name: string; collapsed: boolean; projectIds: string[] }[];
  projectOrdering?: string[];
  defaultShell: string;
  availableShells: ShellConfig[];
  uiFontSize: number;
  terminalFontSize: number;
  layoutSizes?: number[];
  middleColumnSizes?: number[];
  theme: 'auto' | 'light' | 'dark';
  terminalFollowTheme: boolean;
  terminalDisableWebgl?: boolean;
  aiCompletionPopup: boolean;
  aiCompletionTaskbarFlash: boolean;
  vscodePath?: string;
}

export interface ProjectConfig {
  id: string;
  name: string;
  path: string;
  macosBookmark?: string;
  savedLayout?: SavedProjectLayout;
  expandedDirs?: string[];
  lastConversationAt?: number;
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
  | { type: 'leaf'; panes: SavedPane[] }
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

export type AiProvider = 'claude' | 'codex' | 'gemini';

export type PaneStatus = 'idle' | 'ai-complete' | 'ai-thinking' | 'ai-generating' | 'ai-awaiting-input' | 'error';

export interface ProjectState {
  id: string;
  tabs: TerminalTab[];
  activeTabId: string;
  layoutHydrated?: boolean;
  needsAttention?: boolean;
}

export interface AiCompletionNotification {
  id: string;
  projectId: string;
  projectName: string;
  timestamp: number;
}

export interface TerminalTab {
  id: string;
  customTitle?: string;
  splitLayout: SplitNode;
  status: PaneStatus;
}

export type SplitNode =
  | { type: 'leaf'; panes: PaneState[]; activePaneId: string }
  | { type: 'split'; direction: 'horizontal' | 'vertical'; children: SplitNode[]; sizes: number[] };

export interface PaneState {
  id: string;
  shellName: string;
  customTitle?: string;
  status: PaneStatus;
  ptyId: number;
  aiProvider?: AiProvider;
}

// === AI 会话 ===

export interface AiSession {
  id: string;
  sessionType: 'claude' | 'codex' | 'gemini';
  title: string;
  timestamp: string; // ISO 8601
}

// === 文件树 ===

export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
  ignored?: boolean;
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
  provider?: AiProvider;
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

// === Git 历史 ===

export interface GitRepoInfo {
  name: string;
  path: string;
  currentBranch?: string;
}

export interface GitCommitInfo {
  hash: string;
  shortHash: string;
  message: string;
  body?: string;
  author: string;
  timestamp: number;
}

export interface CommitFileInfo {
  path: string;
  status: 'added' | 'modified' | 'deleted' | 'renamed';
  oldPath?: string;
}

export interface BranchInfo {
  name: string;
  isHead: boolean;
  isRemote: boolean;
  commitHash: string;
}
