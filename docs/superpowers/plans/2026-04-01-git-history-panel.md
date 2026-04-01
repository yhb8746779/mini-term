# Git 提交历史面板 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在中列 FileTree 下方新增 Git 提交历史面板，支持扫描项目下所有 git 仓库、二级列表展示提交记录、右键复制 hash / 查看 commit diff。

**Architecture:** 后端用 git2 新增 4 个 Tauri command（发现仓库 / 提交日志 / 提交文件列表 / 单文件 diff）。前端新建 GitHistory 组件（二级列表 + 滚动加载）和 CommitDiffModal 组件（文件选择器 + 复用 InlineView/SideBySideView）。中列用 Allotment vertical 分割 FileTree 和 GitHistory。

**Tech Stack:** Rust + git2 0.19, React 19, TypeScript, Tailwind CSS v4, Allotment, Zustand

**Spec:** `docs/superpowers/specs/2026-04-01-git-history-panel-design.md`

---

## File Structure

### Rust 后端
| 文件 | 动作 | 职责 |
|------|------|------|
| `src-tauri/src/git.rs` | 修改 | 提取 `find_repos()`，新增结构体 `GitRepoInfo` / `GitCommitInfo` / `CommitFileInfo`，新增 4 个 command |
| `src-tauri/src/lib.rs` | 修改 | 注册 4 个新 command |
| `src-tauri/src/config.rs` | 修改 | `AppConfig` 新增 `middle_column_sizes` 字段 |

### 前端
| 文件 | 动作 | 职责 |
|------|------|------|
| `src/types.ts` | 修改 | 新增 `GitRepoInfo` / `GitCommitInfo` / `CommitFileInfo`，`AppConfig` 新增 `middleColumnSizes` |
| `src/utils/timeFormat.ts` | 新建 | 相对时间工具函数 |
| `src/components/GitHistory.tsx` | 新建 | Git 历史面板（二级列表、滚动加载、右键菜单、刷新） |
| `src/components/CommitDiffModal.tsx` | 新建 | Commit diff 弹框（文件选择器 + diff 视图） |
| `src/components/DiffModal.tsx` | 修改 | 导出 `InlineView` 和 `SideBySideView` 供 CommitDiffModal 复用 |
| `src/App.tsx` | 修改 | 中列改为 Allotment vertical，持久化中列分割比例 |

---

### Task 1: 后端 — 提取 find_repos 公共函数 + discover_git_repos command

**Files:**
- Modify: `src-tauri/src/git.rs:100-188` (重构 `get_git_status` 的扫描逻辑)
- Modify: `src-tauri/src/lib.rs:38-39` (注册新 command)

- [ ] **Step 1: 在 git.rs 中新增 GitRepoInfo 结构体和 find_repos 内部函数**

在 `get_git_status` 函数之前添加：

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoInfo {
    pub name: String,
    pub path: String,
}

/// Scan project_path for git repositories.
/// Returns (repo_name, repo_abs_path, Repository) tuples.
fn find_repos(project_path: &Path) -> Vec<(String, PathBuf, Repository)> {
    let mut repos = Vec::new();

    // 1) 项目路径自身是否为仓库（使用 discover 保持向上搜索能力，兼容 get_git_status 原有行为）
    if let Ok(repo) = Repository::discover(project_path) {
        if let Some(workdir) = repo.workdir() {
            let repo_root = workdir.to_path_buf();
            let name = repo_root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "root".to_string());
            repos.push((name, repo_root, repo));
            return repos; // 自身（或祖先）是仓库，不再扫描子目录
        }
    }

    // 2) 扫描一级子目录
    if let Ok(entries) = std::fs::read_dir(project_path) {
        for entry in entries.flatten() {
            let sub = entry.path();
            if sub.is_dir() {
                if let Ok(repo) = Repository::open(&sub) {
                    if let Some(workdir) = repo.workdir() {
                        if workdir.canonicalize().ok() == sub.canonicalize().ok() {
                            let name = sub
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default();
                            repos.push((name, sub, repo));
                        }
                    }
                }
            }
        }
    }

    repos
}
```

- [ ] **Step 2: 重构 get_git_status 使用 find_repos**

将 `get_git_status` 函数体替换为使用 `find_repos`：

```rust
#[tauri::command]
pub fn get_git_status(project_path: String) -> Result<Vec<GitFileStatus>, String> {
    let path = Path::new(&project_path);
    let repos = find_repos(path);

    if repos.is_empty() {
        return Ok(Vec::new());
    }

    let mut all = Vec::new();
    for (_, _, repo) in &repos {
        if let Ok(mut files) = collect_repo_status(repo, Some(path)) {
            all.append(&mut files);
        }
    }
    Ok(all)
}
```

- [ ] **Step 3: 新增 discover_git_repos command**

```rust
#[tauri::command]
pub fn discover_git_repos(project_path: String) -> Result<Vec<GitRepoInfo>, String> {
    let path = Path::new(&project_path);
    let repos = find_repos(path);
    Ok(repos
        .into_iter()
        .map(|(name, abs_path, _)| GitRepoInfo {
            name,
            path: abs_path.to_string_lossy().to_string(),
        })
        .collect())
}
```

- [ ] **Step 4: 在 lib.rs 注册 discover_git_repos**

在 `invoke_handler` 的 `generate_handler!` 宏中，在 `git::get_git_diff,` 后面添加：

```rust
git::discover_git_repos,
```

- [ ] **Step 5: 编译验证**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功，无错误

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/git.rs src-tauri/src/lib.rs
git commit -m "refactor: 提取 find_repos 公共函数，新增 discover_git_repos command"
```

---

### Task 2: 后端 — get_git_log command（cursor 分页）

**Files:**
- Modify: `src-tauri/src/git.rs` (新增 `GitCommitInfo` 结构体和 `get_git_log` command)
- Modify: `src-tauri/src/lib.rs` (注册)

- [ ] **Step 1: 新增 GitCommitInfo 结构体**

在 `GitRepoInfo` 后添加：

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: i64,
}
```

- [ ] **Step 2: 实现 get_git_log command**

```rust
#[tauri::command]
pub fn get_git_log(
    repo_path: String,
    before_commit: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<GitCommitInfo>, String> {
    let path = Path::new(&repo_path);
    let repo = Repository::open(path).map_err(|e| e.to_string())?;
    let limit = limit.unwrap_or(30);

    let mut revwalk = repo.revwalk().map_err(|e| e.to_string())?;
    revwalk.set_sorting(git2::Sort::TIME).map_err(|e| e.to_string())?;

    if let Some(ref hash) = before_commit {
        let oid = git2::Oid::from_str(hash).map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        // Push all parents of the cursor commit
        for parent_id in commit.parent_ids() {
            revwalk.push(parent_id).map_err(|e| e.to_string())?;
        }
    } else {
        revwalk.push_head().map_err(|e| e.to_string())?;
    }

    let mut result = Vec::with_capacity(limit);
    for oid_result in revwalk {
        if result.len() >= limit {
            break;
        }
        let oid = oid_result.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        let hash = oid.to_string();
        let short_hash = hash[..7.min(hash.len())].to_string();
        let message = commit.summary().unwrap_or("").to_string();
        let author = commit.author().name().unwrap_or("unknown").to_string();
        let timestamp = commit.time().seconds();
        result.push(GitCommitInfo {
            hash,
            short_hash,
            message,
            author,
            timestamp,
        });
    }

    Ok(result)
}
```

- [ ] **Step 3: 在 lib.rs 注册 get_git_log**

在 `git::discover_git_repos,` 后添加：

```rust
git::get_git_log,
```

- [ ] **Step 4: 编译验证**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 5: 提交**

```bash
git add src-tauri/src/git.rs src-tauri/src/lib.rs
git commit -m "feat: 新增 get_git_log command，支持 cursor 分页"
```

---

### Task 3: 后端 — get_commit_files + get_commit_file_diff commands

**Files:**
- Modify: `src-tauri/src/git.rs` (新增 `CommitFileInfo` 结构体和 2 个 command)
- Modify: `src-tauri/src/lib.rs` (注册)

- [ ] **Step 1: 新增 CommitFileInfo 结构体**

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileInfo {
    pub path: String,
    pub status: String,
    pub old_path: Option<String>,
}
```

- [ ] **Step 2: 实现 get_commit_files command**

```rust
#[tauri::command]
pub fn get_commit_files(
    repo_path: String,
    commit_hash: String,
) -> Result<Vec<CommitFileInfo>, String> {
    let repo = Repository::open(Path::new(&repo_path)).map_err(|e| e.to_string())?;
    let oid = git2::Oid::from_str(&commit_hash).map_err(|e| e.to_string())?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    let tree = commit.tree().map_err(|e| e.to_string())?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0).map_err(|e| e.to_string())?.tree().map_err(|e| e.to_string())?)
    } else {
        None
    };

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    for delta in diff.deltas() {
        let status = match delta.status() {
            git2::Delta::Added => "added",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Modified => "modified",
            git2::Delta::Renamed => "renamed",
            _ => "modified",
        };
        let path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let old_path = if delta.status() == git2::Delta::Renamed {
            delta.old_file().path().map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        files.push(CommitFileInfo {
            path,
            status: status.to_string(),
            old_path,
        });
    }
    Ok(files)
}
```

- [ ] **Step 3: 实现 get_commit_file_diff command**

```rust
#[tauri::command]
pub fn get_commit_file_diff(
    repo_path: String,
    commit_hash: String,
    file_path: String,
    old_file_path: Option<String>,
) -> Result<GitDiffResult, String> {
    let repo = Repository::open(Path::new(&repo_path)).map_err(|e| e.to_string())?;
    let oid = git2::Oid::from_str(&commit_hash).map_err(|e| e.to_string())?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    let tree = commit.tree().map_err(|e| e.to_string())?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0).map_err(|e| e.to_string())?.tree().map_err(|e| e.to_string())?)
    } else {
        None
    };

    // 获取 new content（当前 commit 中的文件）
    let new_content = match tree.get_path(Path::new(&file_path)) {
        Ok(entry) => {
            let obj = entry.to_object(&repo).map_err(|e| e.to_string())?;
            let blob = obj.as_blob().ok_or("not a blob")?;
            if blob.is_binary() {
                return Ok(GitDiffResult {
                    old_content: String::new(),
                    new_content: String::new(),
                    hunks: Vec::new(),
                    is_binary: true,
                    too_large: false,
                });
            }
            if blob.content().len() > 1_048_576 {
                return Ok(GitDiffResult {
                    old_content: String::new(),
                    new_content: String::new(),
                    hunks: Vec::new(),
                    is_binary: false,
                    too_large: true,
                });
            }
            std::str::from_utf8(blob.content())
                .map_err(|_| "binary".to_string())?
                .to_string()
        }
        Err(_) => String::new(), // file deleted in this commit
    };

    // 获取 old content（parent commit 中的文件，renamed 时使用旧路径）
    let old_lookup_path = old_file_path.as_deref().unwrap_or(&file_path);
    let old_content = if let Some(ref pt) = parent_tree {
        match pt.get_path(Path::new(old_lookup_path)) {
            Ok(entry) => {
                let obj = entry.to_object(&repo).map_err(|e| e.to_string())?;
                let blob = obj.as_blob().ok_or("not a blob")?;
                if blob.is_binary() {
                    return Ok(GitDiffResult {
                        old_content: String::new(),
                        new_content: String::new(),
                        hunks: Vec::new(),
                        is_binary: true,
                        too_large: false,
                    });
                }
                std::str::from_utf8(blob.content())
                    .map_err(|_| "binary".to_string())?
                    .to_string()
            }
            Err(_) => String::new(), // file added in this commit
        }
    } else {
        String::new() // initial commit
    };

    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    let ol = old_lines.len() as u64;
    let nl = new_lines.len() as u64;

    let hunks = if ol * nl > 10_000_000 {
        full_replace_diff(&old_content, &new_content)
    } else {
        build_hunks(&old_lines, &new_lines)
    };

    Ok(GitDiffResult {
        old_content,
        new_content,
        hunks,
        is_binary: false,
        too_large: false,
    })
}
```

- [ ] **Step 4: 在 lib.rs 注册两个新 command**

在 `git::get_git_log,` 后添加：

```rust
git::get_commit_files,
git::get_commit_file_diff,
```

- [ ] **Step 5: 编译验证**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 6: 提交**

```bash
git add src-tauri/src/git.rs src-tauri/src/lib.rs
git commit -m "feat: 新增 get_commit_files 和 get_commit_file_diff commands"
```

---

### Task 4: 后端 — AppConfig 新增 middle_column_sizes 字段

**Files:**
- Modify: `src-tauri/src/config.rs:34-50` (`AppConfig` 结构体)

- [ ] **Step 1: 在 AppConfig 中添加 middle_column_sizes 字段**

在 `config.rs` 的 `AppConfig` 结构体中，`layout_sizes` 字段后添加：

```rust
#[serde(default)]
pub middle_column_sizes: Option<Vec<f64>>,
```

- [ ] **Step 2: 更新 Default 实现**

在 `impl Default for AppConfig` 中添加：

```rust
middle_column_sizes: None,
```

- [ ] **Step 3: 编译验证**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`
Expected: 编译成功

- [ ] **Step 4: 提交**

```bash
git add src-tauri/src/config.rs
git commit -m "feat: AppConfig 新增 middle_column_sizes 字段"
```

---

### Task 5: 前端 — 类型定义 + 相对时间工具

**Files:**
- Modify: `src/types.ts:22-23` (`AppConfig` 接口)
- Create: `src/utils/timeFormat.ts`

- [ ] **Step 1: 在 types.ts 中新增类型**

在 `types.ts` 文件末尾（`FileContentResult` 接口后面）追加：

```typescript
// === Git 历史 ===

export interface GitRepoInfo {
  name: string;
  path: string;
}

export interface GitCommitInfo {
  hash: string;
  shortHash: string;
  message: string;
  author: string;
  timestamp: number;
}

export interface CommitFileInfo {
  path: string;
  status: 'added' | 'modified' | 'deleted' | 'renamed';
  oldPath?: string;
}
```

- [ ] **Step 2: 在 AppConfig 接口中添加 middleColumnSizes**

在 `layoutSizes?: number[];` 后面添加：

```typescript
middleColumnSizes?: number[];
```

- [ ] **Step 3: 创建 src/utils/timeFormat.ts**

```typescript
export function formatRelativeTime(timestamp: number): string {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - timestamp;

  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  if (diff < 2592000) return `${Math.floor(diff / 86400)} 天前`;

  const date = new Date(timestamp * 1000);
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}
```

- [ ] **Step 4: 提交**

```bash
git add src/types.ts src/utils/timeFormat.ts
git commit -m "feat: 新增 Git 历史类型定义和相对时间工具函数"
```

---

### Task 6: 前端 — 导出 DiffModal 子组件

**Files:**
- Modify: `src/components/DiffModal.tsx:16,56` (导出 `InlineView` 和 `SideBySideView`)

- [ ] **Step 1: 导出 InlineView 和 SideBySideView**

将 `DiffModal.tsx` 中两个函数的声明从：

```typescript
function InlineView({ hunks }: { hunks: GitDiffResult['hunks'] }) {
```

改为：

```typescript
export function InlineView({ hunks }: { hunks: GitDiffResult['hunks'] }) {
```

同理：

```typescript
function SideBySideView({ hunks }: { hunks: GitDiffResult['hunks'] }) {
```

改为：

```typescript
export function SideBySideView({ hunks }: { hunks: GitDiffResult['hunks'] }) {
```

- [ ] **Step 2: 提交**

```bash
git add src/components/DiffModal.tsx
git commit -m "refactor: 导出 InlineView 和 SideBySideView 供复用"
```

---

### Task 7: 前端 — GitHistory 组件

**Files:**
- Create: `src/components/GitHistory.tsx`

- [ ] **Step 1: 创建 GitHistory.tsx**

```typescript
import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { useAppStore } from '../store';
import { useTauriEvent } from '../hooks/useTauriEvent';
import { showContextMenu } from '../utils/contextMenu';
import { formatRelativeTime } from '../utils/timeFormat';
import { CommitDiffModal } from './CommitDiffModal';
import type { GitRepoInfo, GitCommitInfo, CommitFileInfo, PtyOutputPayload } from '../types';

interface RepoState {
  commits: GitCommitInfo[];
  loading: boolean;
  hasMore: boolean;
}

const GIT_REFRESH_PATTERNS = [
  /create mode/,
  /Switched to/,
  /Already up to date/,
  /insertions?\(\+\)/,
  /deletions?\(-\)/,
];

export function GitHistory() {
  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const project = config.projects.find((p) => p.id === activeProjectId);

  const [repos, setRepos] = useState<GitRepoInfo[]>([]);
  const [expandedRepos, setExpandedRepos] = useState<Set<string>>(new Set());
  const [repoStates, setRepoStates] = useState<Map<string, RepoState>>(new Map());
  const [diffModal, setDiffModal] = useState<{
    open: boolean;
    repoPath: string;
    commitHash: string;
    commitMessage: string;
    files: CommitFileInfo[];
  } | null>(null);

  const scrollRef = useRef<HTMLDivElement>(null);
  const repoStatesRef = useRef(repoStates);
  repoStatesRef.current = repoStates;

  // 加载仓库列表
  const loadRepos = useCallback(() => {
    if (!project) return;
    invoke<GitRepoInfo[]>('discover_git_repos', { projectPath: project.path })
      .then(setRepos)
      .catch(() => setRepos([]));
  }, [project?.path]);

  useEffect(() => {
    loadRepos();
  }, [loadRepos]);

  // 加载提交历史（通过 ref 读取 repoStates，避免 useCallback 依赖循环）
  const loadCommits = useCallback(
    async (repoPath: string, beforeCommit?: string) => {
      const existing = repoStatesRef.current.get(repoPath);
      if (existing?.loading) return;

      setRepoStates((prev) => {
        const next = new Map(prev);
        const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
        next.set(repoPath, { ...cur, loading: true });
        return next;
      });

      try {
        const commits = await invoke<GitCommitInfo[]>('get_git_log', {
          repoPath,
          beforeCommit: beforeCommit ?? null,
          limit: 30,
        });
        setRepoStates((prev) => {
          const next = new Map(prev);
          const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
          next.set(repoPath, {
            commits: beforeCommit ? [...cur.commits, ...commits] : commits,
            loading: false,
            hasMore: commits.length >= 30,
          });
          return next;
        });
      } catch {
        setRepoStates((prev) => {
          const next = new Map(prev);
          const cur = next.get(repoPath) ?? { commits: [], loading: false, hasMore: true };
          next.set(repoPath, { ...cur, loading: false });
          return next;
        });
      }
    },
    [],
  );

  // 展开/折叠仓库
  const toggleRepo = useCallback(
    (repoPath: string) => {
      setExpandedRepos((prev) => {
        const next = new Set(prev);
        if (next.has(repoPath)) {
          next.delete(repoPath);
        } else {
          next.add(repoPath);
          if (!repoStatesRef.current.has(repoPath)) {
            loadCommits(repoPath);
          }
        }
        return next;
      });
    },
    [loadCommits],
  );

  // 滚动加载（通过 ref 读取最新 state，避免频繁重建）
  const expandedReposRef = useRef(expandedRepos);
  expandedReposRef.current = expandedRepos;

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (el.scrollTop + el.clientHeight < el.scrollHeight - 50) return;

    for (const repoPath of expandedReposRef.current) {
      const state = repoStatesRef.current.get(repoPath);
      if (state && state.hasMore && !state.loading && state.commits.length > 0) {
        const lastHash = state.commits[state.commits.length - 1].hash;
        loadCommits(repoPath, lastHash);
        break; // 一次只加载一个仓库，避免并发过多
      }
    }
  }, [loadCommits]);

  // 右键 — 查看变更
  const handleViewDiff = useCallback(async (repoPath: string, commit: GitCommitInfo) => {
    try {
      const files = await invoke<CommitFileInfo[]>('get_commit_files', {
        repoPath,
        commitHash: commit.hash,
      });
      setDiffModal({
        open: true,
        repoPath,
        commitHash: commit.hash,
        commitMessage: commit.message,
        files,
      });
    } catch (e) {
      console.error('get_commit_files failed:', e);
    }
  }, []);

  // 右键菜单
  const handleCommitContextMenu = useCallback(
    (e: React.MouseEvent, repoPath: string, commit: GitCommitInfo) => {
      e.preventDefault();
      showContextMenu(e.clientX, e.clientY, [
        {
          label: '复制 Commit Hash',
          onClick: () => writeText(commit.hash),
        },
        { separator: true },
        {
          label: '查看变更',
          onClick: () => handleViewDiff(repoPath, commit),
        },
      ]);
    },
    [handleViewDiff],
  );

  // 监听终端 git 操作自动刷新
  const refreshTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const debouncedRefresh = useCallback(() => {
    if (refreshTimerRef.current) clearTimeout(refreshTimerRef.current);
    refreshTimerRef.current = setTimeout(() => {
      loadRepos();
      for (const repoPath of expandedReposRef.current) {
        loadCommits(repoPath);
      }
    }, 500);
  }, [loadRepos, loadCommits]);

  useTauriEvent<PtyOutputPayload>(
    'pty-output',
    useCallback(
      (payload: PtyOutputPayload) => {
        if (GIT_REFRESH_PATTERNS.some((p) => p.test(payload.data))) {
          debouncedRefresh();
        }
      },
      [debouncedRefresh],
    ),
  );

  if (!project) {
    return (
      <div className="h-full bg-[var(--bg-surface)] flex items-center justify-center text-[var(--text-muted)] text-base">
        选择一个项目
      </div>
    );
  }

  return (
    <div className="h-full bg-[var(--bg-surface)] flex flex-col border-l border-[var(--border-subtle)]">
      {/* 头部 */}
      <div className="flex items-center justify-between px-3 pt-3 pb-1.5">
        <span className="text-sm text-[var(--text-muted)] uppercase tracking-[0.12em] font-medium">
          Git History
        </span>
        <button
          className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-sm"
          onClick={() => {
            loadRepos();
            for (const repoPath of expandedRepos) {
              loadCommits(repoPath);
            }
          }}
          title="刷新"
        >
          ↻
        </button>
      </div>

      {/* 内容区 */}
      <div className="flex-1 overflow-y-auto px-1" ref={scrollRef} onScroll={handleScroll}>
        {repos.length === 0 && (
          <div className="text-center text-[var(--text-muted)] text-sm py-6">
            未发现 Git 仓库
          </div>
        )}

        {repos.map((repo) => {
          const isExpanded = expandedRepos.has(repo.path);
          const state = repoStates.get(repo.path);
          return (
            <div key={repo.path}>
              {/* 仓库项 */}
              <div
                className="flex items-center gap-1 py-[5px] px-2 cursor-pointer hover:bg-[var(--border-subtle)] rounded-[var(--radius-sm)] text-base transition-colors duration-100 text-[var(--color-folder)]"
                onClick={() => toggleRepo(repo.path)}
              >
                <span
                  className="text-[13px] w-3 text-center text-[var(--text-muted)] transition-transform duration-150"
                  style={{
                    transform: isExpanded ? 'rotate(0deg)' : 'rotate(-90deg)',
                    display: 'inline-block',
                  }}
                >
                  ▾
                </span>
                <span className="truncate font-medium">{repo.name}</span>
              </div>

              {/* 提交列表 */}
              {isExpanded && (
                <div className="ml-4">
                  {state?.commits.map((commit) => (
                    <div
                      key={commit.hash}
                      className="py-1.5 px-2 cursor-pointer hover:bg-[var(--border-subtle)] rounded-[var(--radius-sm)] transition-colors duration-100"
                      onContextMenu={(e) => handleCommitContextMenu(e, repo.path, commit)}
                      onDoubleClick={() => handleViewDiff(repo.path, commit)}
                    >
                      <div className="text-sm text-[var(--text-primary)] truncate">
                        {commit.message}
                      </div>
                      <div className="text-xs text-[var(--text-muted)] flex items-center gap-1.5 mt-0.5">
                        <span>{commit.author}</span>
                        <span>·</span>
                        <span>{formatRelativeTime(commit.timestamp)}</span>
                        <span>·</span>
                        <span className="font-mono">{commit.shortHash}</span>
                      </div>
                    </div>
                  ))}

                  {state?.loading && (
                    <div className="text-center text-[var(--text-muted)] text-xs py-2">
                      加载中...
                    </div>
                  )}

                  {state && !state.loading && state.commits.length === 0 && (
                    <div className="text-center text-[var(--text-muted)] text-xs py-2">
                      暂无提交
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* Commit Diff Modal */}
      {diffModal && (
        <CommitDiffModal
          open={diffModal.open}
          onClose={() => setDiffModal(null)}
          repoPath={diffModal.repoPath}
          commitHash={diffModal.commitHash}
          commitMessage={diffModal.commitMessage}
          files={diffModal.files}
        />
      )}
    </div>
  );
}
```

- [ ] **Step 2: 提交**

```bash
git add src/components/GitHistory.tsx
git commit -m "feat: 新建 GitHistory 组件（二级仓库/提交列表、滚动加载、右键菜单）"
```

---

### Task 8: 前端 — CommitDiffModal 组件

**Files:**
- Create: `src/components/CommitDiffModal.tsx`

- [ ] **Step 1: 创建 CommitDiffModal.tsx**

```typescript
import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { InlineView, SideBySideView } from './DiffModal';
import type { CommitFileInfo, GitDiffResult } from '../types';

interface CommitDiffModalProps {
  open: boolean;
  onClose: () => void;
  repoPath: string;
  commitHash: string;
  commitMessage: string;
  files: CommitFileInfo[];
}

type ViewMode = 'side-by-side' | 'inline';

const STATUS_LABELS: Record<string, { text: string; color: string }> = {
  added: { text: 'A', color: 'text-green-400' },
  modified: { text: 'M', color: 'text-amber-400' },
  deleted: { text: 'D', color: 'text-red-400' },
  renamed: { text: 'R', color: 'text-blue-400' },
};

export function CommitDiffModal({
  open,
  onClose,
  repoPath,
  commitHash,
  commitMessage,
  files,
}: CommitDiffModalProps) {
  const [viewMode, setViewMode] = useState<ViewMode>('side-by-side');
  const [selectedFile, setSelectedFile] = useState<string>(files[0]?.path ?? '');
  const [diffResult, setDiffResult] = useState<GitDiffResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  // 加载选中文件的 diff
  const loadDiff = useCallback(
    async (filePath: string) => {
      if (!filePath) return;
      setLoading(true);
      setError('');
      setDiffResult(null);
      const fileInfo = files.find((f) => f.path === filePath);
      try {
        const result = await invoke<GitDiffResult>('get_commit_file_diff', {
          repoPath,
          commitHash,
          filePath,
          oldFilePath: fileInfo?.oldPath ?? null,
        });
        setDiffResult(result);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    },
    [repoPath, commitHash, files],
  );

  useEffect(() => {
    if (open && selectedFile) {
      loadDiff(selectedFile);
    }
  }, [open, selectedFile, loadDiff]);

  // 文件切换时自动选择第一个
  useEffect(() => {
    if (open && files.length > 0 && !files.find((f) => f.path === selectedFile)) {
      setSelectedFile(files[0].path);
    }
  }, [open, files, selectedFile]);

  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose]);

  if (!open) return null;

  const shortHash = commitHash.slice(0, 7);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center" onClick={onClose}>
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
      <div
        className="relative flex overflow-hidden bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-2xl animate-slide-in"
        style={{ width: '92vw', height: '85vh' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 左侧文件列表 */}
        <div className="w-56 flex-shrink-0 border-r border-[var(--border-subtle)] flex flex-col bg-[var(--bg-elevated)]">
          <div className="px-3 py-3 border-b border-[var(--border-subtle)]">
            <div className="text-sm font-medium text-[var(--accent)] truncate">
              {commitMessage}
            </div>
            <div className="text-xs text-[var(--text-muted)] mt-1 font-mono">{shortHash}</div>
          </div>
          <div className="flex-1 overflow-y-auto">
            {files.map((file) => {
              const label = STATUS_LABELS[file.status] ?? { text: '?', color: 'text-[var(--text-muted)]' };
              const fileName = file.path.split('/').pop() ?? file.path;
              const isSelected = file.path === selectedFile;
              return (
                <div
                  key={file.path}
                  className={`flex items-center gap-2 px-3 py-1.5 cursor-pointer text-sm transition-colors ${
                    isSelected
                      ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                      : 'text-[var(--text-primary)] hover:bg-[var(--border-subtle)]'
                  }`}
                  onClick={() => setSelectedFile(file.path)}
                  title={file.path}
                >
                  <span className={`text-xs font-bold flex-shrink-0 ${label.color}`}>
                    {label.text}
                  </span>
                  <span className="truncate">{fileName}</span>
                </div>
              );
            })}
          </div>
          <div className="px-3 py-2 border-t border-[var(--border-subtle)] text-xs text-[var(--text-muted)]">
            {files.length} 个文件变更
          </div>
        </div>

        {/* 右侧 diff 区域 */}
        <div className="flex-1 flex flex-col">
          {/* 工具栏 */}
          <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0">
            <div className="flex items-center gap-2">
              <span className="text-sm text-[var(--text-primary)] truncate max-w-[400px]">
                {selectedFile}
              </span>
            </div>
            <div className="flex items-center gap-2">
              <div className="flex rounded-[var(--radius-sm)] border border-[var(--border-default)] overflow-hidden">
                <button
                  className={`px-3 py-1 text-sm transition-colors ${
                    viewMode === 'side-by-side'
                      ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                  onClick={() => setViewMode('side-by-side')}
                >
                  并排
                </button>
                <button
                  className={`px-3 py-1 text-sm transition-colors ${
                    viewMode === 'inline'
                      ? 'bg-[var(--accent-subtle)] text-[var(--accent)]'
                      : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'
                  }`}
                  onClick={() => setViewMode('inline')}
                >
                  内联
                </button>
              </div>
              <button
                className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-lg leading-none ml-2"
                onClick={onClose}
              >
                ✕
              </button>
            </div>
          </div>

          {/* Diff 内容 */}
          <div className="flex-1 overflow-auto bg-[var(--bg-base)]">
            {loading && (
              <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
                加载中...
              </div>
            )}
            {error && (
              <div className="flex items-center justify-center h-full text-[var(--color-error)]">
                {error}
              </div>
            )}
            {diffResult && diffResult.isBinary && (
              <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
                二进制文件，不支持 diff 预览
              </div>
            )}
            {diffResult && diffResult.tooLarge && (
              <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
                文件过大（&gt;1MB），不支持 diff 预览
              </div>
            )}
            {diffResult && !diffResult.isBinary && !diffResult.tooLarge && (
              viewMode === 'side-by-side'
                ? <SideBySideView hunks={diffResult.hunks} />
                : <InlineView hunks={diffResult.hunks} />
            )}
            {!loading && !error && !diffResult && files.length === 0 && (
              <div className="flex items-center justify-center h-full text-[var(--text-muted)]">
                该提交无文件变更
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 提交**

```bash
git add src/components/CommitDiffModal.tsx
git commit -m "feat: 新建 CommitDiffModal 组件（文件选择器 + diff 视图）"
```

---

### Task 9: 前端 — App.tsx 中列改为垂直分割

**Files:**
- Modify: `src/App.tsx:4,120-122` (导入 GitHistory，中列改为 Allotment vertical)

- [ ] **Step 1: 添加 GitHistory 导入**

在 `App.tsx` 顶部 import 区域添加：

```typescript
import { GitHistory } from './components/GitHistory';
```

- [ ] **Step 2: 新增中列分割比例保存逻辑**

在 `saveLayoutSizes` 的 `useCallback` 后面添加：

```typescript
const saveMidTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
const saveMiddleColumnSizes = useCallback((sizes: number[]) => {
  clearTimeout(saveMidTimer.current);
  saveMidTimer.current = setTimeout(() => {
    const cfg = useAppStore.getState().config;
    const newConfig = { ...cfg, middleColumnSizes: sizes };
    setConfig(newConfig);
    invoke('save_config', { config: newConfig });
  }, 500);
}, [setConfig]);
```

- [ ] **Step 3: 替换中列 Pane 内容**

将 `App.tsx` 中：

```tsx
<Allotment.Pane minSize={180}>
  <FileTree key={activeProjectId} />
</Allotment.Pane>
```

替换为：

```tsx
<Allotment.Pane minSize={180}>
  <Allotment
    vertical
    defaultSizes={config.middleColumnSizes ?? [300, 200]}
    onChange={saveMiddleColumnSizes}
  >
    <Allotment.Pane minSize={150}>
      <FileTree key={activeProjectId} />
    </Allotment.Pane>
    <Allotment.Pane minSize={100}>
      <GitHistory key={activeProjectId} />
    </Allotment.Pane>
  </Allotment>
</Allotment.Pane>
```

- [ ] **Step 4: 运行 tauri dev 验证**

Run: `npm run tauri dev`
Expected: 应用启动，中列显示 FileTree 和 GitHistory 上下分割，可拖拽分割线

- [ ] **Step 5: 提交**

```bash
git add src/App.tsx
git commit -m "feat: 中列改为垂直分割布局（FileTree + GitHistory）"
```

---

### Task 10: 集成验证

- [ ] **Step 1: 启动完整应用**

Run: `npm run tauri dev`

- [ ] **Step 2: 验证功能**

逐项验证：
1. 打开一个包含 git 仓库的项目 → GitHistory 面板显示仓库名
2. 展开仓库 → 显示最近 30 条提交（消息 + 作者 + 相对时间 + 短hash）
3. 滚动到底 → 自动加载更多提交
4. 右键提交 → 弹出菜单（复制 hash / 查看变更）
5. 点击"复制 Commit Hash" → 粘贴验证正确
6. 点击"查看变更" → CommitDiffModal 弹出，左侧文件列表，右侧 diff
7. 切换文件 → diff 内容更新
8. 并排/内联视图切换正常
9. 中列拖拽分割线 → 比例变化，重启后保留
10. 刷新按钮点击 → 仓库和提交列表刷新
11. 在终端执行 git commit → GitHistory 自动刷新

- [ ] **Step 3: 提交最终调整（如有）**

```bash
git add -A
git commit -m "fix: Git 历史面板集成调整"
```
