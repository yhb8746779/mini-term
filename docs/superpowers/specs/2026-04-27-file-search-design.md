# 文件搜索功能设计

## 概述

为 mini-term 新增全局文件搜索功能，支持文件名搜索和文件内容搜索，以独立弹窗面板呈现，Rust 后端流式推送搜索结果。

## 需求

- 文件名搜索 + 文件内容搜索，可切换模式
- 独立弹窗面板（非 Command Palette 风格）
- 默认纯文本匹配，可选正则表达式
- 尊重多层级 `.gitignore` + 内置忽略列表（`ALWAYS_IGNORE`）
- 手动触发（回车 / 点击按钮）
- 双入口：快捷键 `Ctrl+Shift+F` + FileTree 工具栏按钮

## 后端架构

### 新增文件

`src-tauri/src/search.rs`

### 依赖

使用 `ignore` crate（ripgrep 底层库），天然支持多层级 `.gitignore` 解析。

### Tauri Commands

| Command | 参数 | 说明 |
|---------|------|------|
| `start_search` | `project_root, query, mode, use_regex, search_id` | 启动异步搜索，立即返回 |
| `cancel_search` | `search_id` | 取消进行中的搜索 |

### Tauri Events

| Event | Payload | 说明 |
|-------|---------|------|
| `search-results` | `{ search_id, items: SearchResultItem[] }` | 批量推送结果（每 50 条或每 100ms） |
| `search-complete` | `{ search_id, total_count, cancelled }` | 搜索结束 |

### 数据结构

```rust
enum SearchMode {
    FileName,
    FileContent,
}

struct SearchResultItem {
    file_path: String,           // 相对于 project_root
    file_name: String,
    line_number: Option<u32>,    // 内容搜索
    line_content: Option<String>,// 匹配行文本
    match_ranges: Vec<(usize, usize)>, // 行内匹配位置 (start, end)
}
```

### 搜索生命周期管理

- 用 `Arc<AtomicBool>` 作为取消标记，存入 `HashMap<search_id, CancelFlag>`
- `start_search` 在 `tokio::spawn` 中执行，每处理一个文件检查取消标记
- `cancel_search` 设置标记，搜索线程退出并发送 `search-complete(cancelled: true)`
- 新搜索启动时自动取消同一项目的旧搜索

### 搜索逻辑

- **文件名搜索**：`ignore::WalkBuilder` 遍历文件树，对每个文件名做子串/正则匹配
- **内容搜索**：遍历文件后读取内容，跳过二进制文件，逐行匹配，返回匹配行号和行内容
- 二进制文件检测：读取前 8KB 检查是否包含 NULL 字节

## 前端设计

### 新增组件

`src/components/SearchModal.tsx`

### UI 布局

- 顶部：标题 + 关闭按钮
- 模式切换：「文件名 / 内容」tab 按钮
- 搜索栏：输入框 + `.*` 正则切换按钮
- 结果列表：
  - 文件名模式：文件名（关键词高亮）+ 相对路径
  - 内容模式：按文件分组，显示行号 + 匹配行内容（关键词高亮）
- 底部状态栏：匹配统计（"找到 N 个文件" / "N 个文件中找到 M 处匹配"）
- 搜索中显示 loading 动画

### 组件状态（本地 state）

```typescript
interface SearchState {
  query: string;
  mode: 'filename' | 'content';
  useRegex: boolean;
  searchId: string | null;
  results: SearchResultItem[];
  status: 'idle' | 'searching' | 'done';
  totalCount: number;
}
```

不放入全局 store，弹窗关闭即清空。

### 结果上限

前端累积超过 1000 条时停止渲染新结果并提示，后端继续计数到 `search-complete` 给出总数。

### 结果交互

- 单击：打开 FileViewerModal 预览，内容搜索传入 `highlightLine` 参数跳到匹配行
- 双击 / 回车：调用已有的编辑器打开逻辑

## 入口与快捷键

### 快捷键

- `Ctrl+Shift+F`：打开/关闭搜索弹窗
- 弹窗内 `Escape`：关闭弹窗
- 弹窗内 `Enter`：触发搜索

### FileTree 工具栏

在现有刷新按钮旁新增搜索图标按钮。

### 组件挂载

SearchModal 挂载在 App.tsx 层级（与 FileViewerModal 平级），通过 `open/close` 状态控制显隐。

## 数据流

```
用户按 Ctrl+Shift+F → 打开 SearchModal
输入关键词 + 回车 → invoke('start_search', { projectRoot, query, mode, useRegex, searchId })
                    → Rust spawn 搜索线程
                    → 前端监听 search-results / search-complete
                    → 结果逐批渲染
用户改关键词再搜 → cancel_search(旧id) → start_search(新id)
单击结果 → FileViewerModal（跳到匹配行）
双击/回车 → 编辑器打开
关闭弹窗 → cancel 进行中搜索，清理状态
```
