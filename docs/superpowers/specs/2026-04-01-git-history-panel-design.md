# Git 提交历史面板设计

## 概述

在 mini-term 中列（FileTree 下方）新增 Git 提交历史面板，支持自动扫描项目下所有 git 仓库，以二级列表展示提交记录。

## 需求

- 自动扫描项目目录下所有 git 仓库（包括项目本身就是 git 仓库的情况）
- 二级列表：一级为仓库名，展开后二级为提交记录
- 每条提交显示：提交消息 + 作者 + 相对时间 + 短 hash
- 右键菜单：复制 commit hash、查看该提交的 diff
- 查看 diff 使用新建的 CommitDiffModal 组件，支持多文件切换
- 滚动到底自动加载更多（每次 30 条，cursor 分页）

## 技术方案：纯 git2 实现

与现有 git.rs 一致的技术栈，不引入外部依赖。

## 后端设计（Rust）

### 新增数据结构

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoInfo {
    pub name: String,      // 仓库目录名
    pub path: String,      // 仓库绝对路径
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,          // 完整 hash
    pub short_hash: String,    // 前 7 位
    pub message: String,       // 首行提交消息
    pub author: String,        // 作者名
    pub timestamp: i64,        // Unix 时间戳
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileInfo {
    pub path: String,
    pub status: String,            // "added" | "modified" | "deleted" | "renamed"
    pub old_path: Option<String>,
}
```

### 新增 Tauri Commands

#### 1. `discover_git_repos(project_path: String) -> Vec<GitRepoInfo>`

- 使用 `Repository::open()`（不用 `discover()`，避免向上搜索祖先目录）
- 先检查 project_path 本身是否为 git 仓库
- 再扫描一级子目录，尝试 `open()` 每个子目录
- 提取公共的 `find_repos()` 内部函数，与 `get_git_status` 共享扫描逻辑
- 返回仓库名称 + 绝对路径

#### 2. `get_git_log(repo_path: String, before_commit: Option<String>, limit: usize) -> Vec<GitCommitInfo>`

- 用 `git2::Revwalk` 遍历提交历史
- 使用 cursor 分页：`before_commit` 为上一批最后一条的 hash，从其 parent 开始遍历
- 首次加载传 `None`，从 HEAD 开始
- `limit` 默认 30
- 返回提交列表，按时间倒序

#### 3. `get_commit_files(repo_path: String, commit_hash: String) -> Vec<CommitFileInfo>`

- 解析 commit hash 找到对应 commit
- 对比该 commit 与其第一个 parent 的 tree diff
- 初始提交（无 parent）：所有文件视为 added
- 仅返回变更文件列表（路径 + 状态），不含 diff 内容

#### 4. `get_commit_file_diff(repo_path: String, commit_hash: String, file_path: String) -> GitDiffResult`

- 获取指定 commit 中指定文件的 diff 内容
- 复用现有 `GitDiffResult` / `DiffHunk` / `DiffLine` 结构
- 复用现有的大文件保护（>1MB）和二进制检测逻辑

拆分为两步调用（先列文件再取 diff）可避免大型 commit 一次返回所有文件 diff 的性能问题。

### 修改文件

- `src-tauri/src/git.rs`：提取 `find_repos()` 公共函数，新增 4 个 command + 结构体
- `src-tauri/src/lib.rs`：注册新 command

## 前端设计

### 布局变更

App.tsx 中列从单独的 `<FileTree>` 改为 `<Allotment vertical>`：

```
中列 (Pane 2)
├── FileTree（上，minSize=150）
├── ── 可拖拽分割线 ──
└── GitHistory（下，minSize=100）
```

新增 `config.middleColumnSizes: number[]` 字段存储中列内部的垂直分割比例，与主布局的 `layoutSizes` 独立。需同步更新 Rust 端 `AppConfig` 结构体和 `save_config` / `load_config`。

### 新建 GitHistory 组件

```
GitHistory
├── 头部 "Git History"（含刷新按钮）
├── 仓库列表（可滚动）
│   ├── RepoItem（一级：仓库名 + 展开箭头）
│   │   └── CommitList（二级：展开后的提交列表）
│   │       ├── CommitItem × N
│   │       │   ├── 首行：提交消息（截断）
│   │       │   ├── 次行：作者 · 相对时间 · 短hash
│   │       │   └── 右键菜单：复制hash / 查看diff
│   │       └── 滚动到底 → 自动加载下一批 30 条
│   └── RepoItem ...
└── 空状态："未发现 Git 仓库"
```

状态管理采用组件本地 state + `key={activeProjectId}` 强制重建（与 FileTree 一致）。

### 新建 CommitDiffModal 组件

不修改现有 DiffModal，新建 `CommitDiffModal` 组件：

- 接收 `CommitFileInfo[]`（变更文件列表）+ `repoPath` + `commitHash`
- 左侧：文件列表（显示路径 + 状态标签）
- 右侧：选中文件的 diff 内容（复用 `InlineView` / `SideBySideView` 子组件）
- 选择文件时按需调用 `get_commit_file_diff` 获取 diff

### 数据流

1. 切换项目时调用 `discover_git_repos` 获取仓库列表
2. 展开仓库时调用 `get_git_log(path, None, 30)` 加载首批
3. 滚动到底调用 `get_git_log(path, lastCommitHash, 30)` 加载更多
4. 右键 → "查看变更" → 调用 `get_commit_files` → 弹出 CommitDiffModal
5. CommitDiffModal 中选择文件 → 调用 `get_commit_file_diff` → 渲染 diff

### 刷新机制

- 头部提供手动刷新按钮
- 监听 `pty-output` 事件，匹配 git 关键词（与 FileTree 一致），自动刷新已展开仓库的 commit 列表

### 持久化

- `config.middleColumnSizes`：中列 FileTree / GitHistory 的分割比例
- `config.expandedGitRepos`：每个项目已展开的仓库路径列表（类似 `expandedDirs`）

### 类型定义（types.ts 新增）

```typescript
interface GitRepoInfo {
  name: string
  path: string
}

interface GitCommitInfo {
  hash: string
  shortHash: string
  message: string
  author: string
  timestamp: number
}

interface CommitFileInfo {
  path: string
  status: 'added' | 'modified' | 'deleted' | 'renamed'
  oldPath?: string
}
```

### 相对时间

放在 `src/utils/timeFormat.ts` 作为共享工具函数：刚刚 / N分钟前 / N小时前 / N天前 / 具体日期。

## 边界情况

| 场景 | 处理 |
|------|------|
| 项目路径本身就是 git 仓库 | `discover_git_repos` 返回自身，列表只有一项 |
| 空仓库（无提交） | `get_git_log` 返回空数组，显示"暂无提交" |
| 无 git 仓库 | 显示"未发现 Git 仓库" |
| merge commit（多 parent） | diff 对比第一个 parent |
| 初始提交（无 parent） | 所有文件视为 added，old content 为空 |
| 大型 commit（数百文件） | 先返回文件列表，按需加载单文件 diff |
| detached HEAD | 正常遍历 commit 历史，不影响功能 |

## 需要修改的文件清单

| 文件 | 改动 |
|------|------|
| `src-tauri/src/git.rs` | 提取 `find_repos()`，新增 4 个 command + 结构体 |
| `src-tauri/src/lib.rs` | 注册新 command |
| `src-tauri/src/config.rs` | `AppConfig` 新增 `middle_column_sizes` 字段 |
| `src/types.ts` | 新增类型定义 + `AppConfig` 新增 `middleColumnSizes` |
| `src/App.tsx` | 中列改为垂直 Allotment，持久化中列分割比例 |
| `src/store.ts` | 新增 `expandedGitRepos` 持久化 |
| `src/utils/timeFormat.ts`（新建） | 相对时间工具函数 |
| `src/components/GitHistory.tsx`（新建） | Git 历史面板组件 |
| `src/components/CommitDiffModal.tsx`（新建） | Commit Diff 查看弹框 |
