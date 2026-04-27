# 文件搜索功能 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add global file search (filename + content) with streaming results to mini-term.

**Architecture:** Rust backend walks the file tree using the `ignore` crate (respects `.gitignore`), matches filenames or file contents, and streams results to the frontend via Tauri events in batches (50 items or 100ms). React frontend renders a SearchModal with search input, mode toggle, regex toggle, and scrollable results. Cancel support via `Arc<AtomicBool>`.

**Tech Stack:** Rust (`ignore` 0.4, `regex`), Tauri v2 events + commands, React 19, TypeScript, Tailwind CSS

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src-tauri/src/search.rs` | Search types, SearchManager, search logic, Tauri commands |
| Modify | `src-tauri/src/lib.rs` | Register `mod search`, `.manage(SearchManager)`, commands |
| Modify | `src-tauri/Cargo.toml` | Add `regex` dependency |
| Modify | `src-tauri/src/fs.rs:66` | Make `ALWAYS_IGNORE` pub |
| Modify | `src/types.ts` | Add `SearchResultItem`, event payload types |
| Modify | `src/store.ts` | Add `searchModalOpen` boolean + setter |
| Create | `src/components/SearchModal.tsx` | Search modal UI (input, results, highlighting) |
| Modify | `src/components/FileViewerModal.tsx` | Add `highlightLine` prop with scroll-to |
| Modify | `src/App.tsx` | Mount SearchModal, register `Ctrl+Shift+F` shortcut |
| Modify | `src/components/FileTree.tsx` | Add search icon button to toolbar |

---

### Task 1: Rust — Search module data structures and helpers

**Files:**
- Create: `src-tauri/src/search.rs`
- Modify: `src-tauri/Cargo.toml:20-33`
- Modify: `src-tauri/src/fs.rs:66`

- [ ] **Step 1: Add `regex` dependency to Cargo.toml**

In `src-tauri/Cargo.toml`, add after line 28 (`ignore = "0.4"`):

```toml
regex = "1"
```

- [ ] **Step 2: Make `ALWAYS_IGNORE` pub in fs.rs**

In `src-tauri/src/fs.rs:66`, change:

```rust
const ALWAYS_IGNORE: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "__pycache__", ".superpowers"];
```

to:

```rust
pub const ALWAYS_IGNORE: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "__pycache__", ".superpowers"];
```

- [ ] **Step 3: Create search.rs with data structures**

Create `src-tauri/src/search.rs` with:

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

// ── Data structures ──

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SearchMode {
    FileName,
    FileContent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultItem {
    pub file_path: String,
    pub file_name: String,
    pub line_number: Option<u32>,
    pub line_content: Option<String>,
    pub match_ranges: Vec<(usize, usize)>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResultsPayload {
    search_id: String,
    items: Vec<SearchResultItem>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchCompletePayload {
    search_id: String,
    total_count: u32,
    cancelled: bool,
}

// ── SearchManager ──

#[derive(Clone)]
pub struct SearchManager {
    // search_id → (project_root, cancel_flag)
    active_searches: Arc<Mutex<HashMap<String, (String, Arc<AtomicBool>)>>>,
}

impl SearchManager {
    pub fn new() -> Self {
        Self {
            active_searches: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register(&self, search_id: &str, project_root: &str) -> Arc<AtomicBool> {
        let mut searches = self.active_searches.lock().unwrap();
        // Cancel all existing searches for the same project
        let to_cancel: Vec<String> = searches
            .iter()
            .filter(|(_, (root, _))| root == project_root)
            .map(|(id, _)| id.clone())
            .collect();
        for id in to_cancel {
            if let Some((_, flag)) = searches.remove(&id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        let flag = Arc::new(AtomicBool::new(false));
        searches.insert(
            search_id.to_string(),
            (project_root.to_string(), flag.clone()),
        );
        flag
    }

    pub fn cancel(&self, search_id: &str) {
        let mut searches = self.active_searches.lock().unwrap();
        if let Some((_, flag)) = searches.remove(search_id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn remove(&self, search_id: &str) {
        self.active_searches.lock().unwrap().remove(search_id);
    }
}

// ── Helpers ──

fn is_binary(data: &[u8]) -> bool {
    data.iter().take(8192).any(|&b| b == 0)
}

fn build_walker(root: &str) -> ignore::Walk {
    let mut builder = ignore::WalkBuilder::new(root);
    builder.hidden(false);
    builder.filter_entry(|entry| {
        if entry.file_type().map_or(false, |ft| ft.is_dir()) {
            let name = entry.file_name().to_str().unwrap_or("");
            !crate::fs::ALWAYS_IGNORE.contains(&name)
        } else {
            true
        }
    });
    builder.build()
}

fn find_substring_matches(text: &str, query_lower: &str) -> Vec<(usize, usize)> {
    let text_lower = text.to_lowercase();
    let mut result = Vec::new();
    let mut start = 0;
    while start < text_lower.len() {
        match text_lower[start..].find(query_lower) {
            Some(pos) => {
                let abs_start = start + pos;
                let abs_end = abs_start + query_lower.len();
                result.push((abs_start, abs_end));
                start = abs_start + 1;
            }
            None => break,
        }
    }
    result
}

fn find_regex_matches(text: &str, re: &Regex) -> Vec<(usize, usize)> {
    re.find_iter(text).map(|m| (m.start(), m.end())).collect()
}

/// Rust str::find returns byte offsets, but JS String.slice uses UTF-16 code unit offsets.
/// Convert byte-offset ranges to char-index ranges so the frontend HighlightText works
/// correctly on non-ASCII text (CJK, emoji, etc.).
fn byte_ranges_to_char_ranges(text: &str, byte_ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if byte_ranges.is_empty() {
        return byte_ranges;
    }
    // Pre-build byte-offset → char-index lookup table (one pass)
    let mut byte_to_char = vec![0usize; text.len() + 1];
    for (ci, (bi, _)) in text.char_indices().enumerate() {
        byte_to_char[bi] = ci;
    }
    let total_chars = text.chars().count();
    byte_to_char[text.len()] = total_chars;
    byte_ranges
        .into_iter()
        .map(|(s, e)| (byte_to_char[s], byte_to_char[e]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_manager_register_and_cancel() {
        let mgr = SearchManager::new();
        let flag = mgr.register("s1", "/project");
        assert!(!flag.load(Ordering::Relaxed));
        mgr.cancel("s1");
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn search_manager_auto_cancels_same_project() {
        let mgr = SearchManager::new();
        let flag1 = mgr.register("s1", "/project");
        let _flag2 = mgr.register("s2", "/project");
        // s1 should be auto-cancelled
        assert!(flag1.load(Ordering::Relaxed));
    }

    #[test]
    fn search_manager_different_projects_independent() {
        let mgr = SearchManager::new();
        let flag1 = mgr.register("s1", "/project-a");
        let _flag2 = mgr.register("s2", "/project-b");
        assert!(!flag1.load(Ordering::Relaxed));
    }

    #[test]
    fn is_binary_detects_null_bytes() {
        assert!(is_binary(&[0x48, 0x65, 0x00, 0x6c]));
        assert!(!is_binary(b"Hello world"));
        assert!(!is_binary(b""));
    }

    #[test]
    fn find_substring_case_insensitive() {
        let matches = find_substring_matches("Hello World hello", "hello");
        assert_eq!(matches, vec![(0, 5), (12, 17)]);
    }

    #[test]
    fn find_substring_no_match() {
        let matches = find_substring_matches("foo bar", "baz");
        assert!(matches.is_empty());
    }

    #[test]
    fn find_regex_matches_basic() {
        let re = Regex::new(r"\d+").unwrap();
        let matches = find_regex_matches("abc123def456", &re);
        assert_eq!(matches, vec![(3, 6), (9, 12)]);
    }

    #[test]
    fn byte_to_char_ranges_ascii() {
        let ranges = byte_ranges_to_char_ranges("hello", vec![(0, 5)]);
        assert_eq!(ranges, vec![(0, 5)]);
    }

    #[test]
    fn byte_to_char_ranges_cjk() {
        // "你好world" — "你" = 3 bytes, "好" = 3 bytes, "world" = 5 bytes
        let text = "你好world";
        // byte offsets for "world": starts at byte 6, ends at byte 11
        let ranges = byte_ranges_to_char_ranges(text, vec![(6, 11)]);
        // char offsets for "world": starts at char 2, ends at char 7
        assert_eq!(ranges, vec![(2, 7)]);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd src-tauri && cargo test --lib search::tests -- --nocapture`

Expected: All 9 tests pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/search.rs src-tauri/Cargo.toml src-tauri/src/fs.rs
git commit -m "feat(search): 搜索模块基础结构

- 新建 search.rs，定义 SearchMode、SearchResultItem、事件 payload 数据结构
- SearchManager 管理活跃搜索的取消标记，新搜索自动取消同项目旧搜索
- 辅助函数：is_binary 检测二进制、build_walker 构建遵循 .gitignore 的文件遍历器、
  find_substring_matches / find_regex_matches 文本匹配、
  byte_ranges_to_char_ranges 字节偏移转字符偏移（支持 CJK 等多字节文本高亮）
- fs.rs 的 ALWAYS_IGNORE 改为 pub 供 search 模块复用
- Cargo.toml 新增 regex 依赖
- 9 项单元测试全部通过"
```

---

### Task 2: Rust — Search logic and Tauri commands

**Files:**
- Modify: `src-tauri/src/search.rs` (append after helpers, before `#[cfg(test)]`)
- Modify: `src-tauri/src/lib.rs:1,28-29,58-95`

- [ ] **Step 1: Add ResultBatcher and search functions to search.rs**

Insert before `#[cfg(test)]` in `src-tauri/src/search.rs`:

```rust
// ── Result batching ──

struct ResultBatcher {
    buffer: Vec<SearchResultItem>,
    last_flush: Instant,
    app: AppHandle,
    search_id: String,
    total_count: u32,
}

impl ResultBatcher {
    fn new(app: AppHandle, search_id: String) -> Self {
        Self {
            buffer: Vec::new(),
            last_flush: Instant::now(),
            app,
            search_id,
            total_count: 0,
        }
    }

    fn push(&mut self, item: SearchResultItem) {
        self.total_count += 1;
        self.buffer.push(item);
        if self.buffer.len() >= 50 || self.last_flush.elapsed() >= Duration::from_millis(100) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.buffer);
        let _ = self.app.emit(
            "search-results",
            SearchResultsPayload {
                search_id: self.search_id.clone(),
                items,
            },
        );
        self.last_flush = Instant::now();
    }

    fn finish(mut self, cancelled: bool) {
        self.flush();
        let _ = self.app.emit(
            "search-complete",
            SearchCompletePayload {
                search_id: self.search_id.clone(),
                total_count: self.total_count,
                cancelled,
            },
        );
    }
}

// ── Search functions ──

fn search_filenames(
    root: &str,
    query: &str,
    use_regex: bool,
    cancel: &AtomicBool,
    batcher: &mut ResultBatcher,
) -> Result<(), String> {
    let re = if use_regex {
        Some(Regex::new(query).map_err(|e| format!("Invalid regex: {}", e))?)
    } else {
        None
    };
    let query_lower = query.to_lowercase();

    for entry in build_walker(root) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map_or(true, |ft| ft.is_dir()) {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();

        let matches = if let Some(ref re) = re {
            find_regex_matches(&file_name, re)
        } else {
            find_substring_matches(&file_name, &query_lower)
        };

        if !matches.is_empty() {
            let char_ranges = byte_ranges_to_char_ranges(&file_name, matches);
            let rel_path = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            batcher.push(SearchResultItem {
                file_path: rel_path,
                file_name,
                line_number: None,
                line_content: None,
                match_ranges: char_ranges,
            });
        }
    }
    Ok(())
}

fn search_contents(
    root: &str,
    query: &str,
    use_regex: bool,
    cancel: &AtomicBool,
    batcher: &mut ResultBatcher,
) -> Result<(), String> {
    let re = if use_regex {
        Some(Regex::new(query).map_err(|e| format!("Invalid regex: {}", e))?)
    } else {
        None
    };
    let query_lower = query.to_lowercase();

    for entry in build_walker(root) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().map_or(true, |ft| ft.is_dir()) {
            continue;
        }

        let path = entry.path();
        let content = match std::fs::read(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if is_binary(&content) {
            continue;
        }
        let text = match String::from_utf8(content) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let file_name = entry.file_name().to_string_lossy().to_string();
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        for (line_idx, line) in text.lines().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let matches = if let Some(ref re) = re {
                find_regex_matches(line, re)
            } else {
                find_substring_matches(line, &query_lower)
            };
            if !matches.is_empty() {
                let char_ranges = byte_ranges_to_char_ranges(line, matches);
                batcher.push(SearchResultItem {
                    file_path: rel_path.clone(),
                    file_name: file_name.clone(),
                    line_number: Some((line_idx + 1) as u32),
                    line_content: Some(line.to_string()),
                    match_ranges: char_ranges,
                });
            }
        }
    }
    Ok(())
}

// ── Tauri commands ──

#[tauri::command]
pub fn start_search(
    app: AppHandle,
    state: tauri::State<'_, SearchManager>,
    project_root: String,
    query: String,
    mode: String,
    use_regex: bool,
    search_id: String,
) -> Result<(), String> {
    if query.is_empty() {
        return Err("Search query is empty".to_string());
    }
    if use_regex {
        Regex::new(&query).map_err(|e| format!("Invalid regex: {}", e))?;
    }

    let manager = state.inner().clone();
    let cancel = manager.register(&search_id, &project_root);
    let search_mode = match mode.as_str() {
        "content" => SearchMode::FileContent,
        _ => SearchMode::FileName,
    };

    let sid = search_id.clone();
    std::thread::spawn(move || {
        let mut batcher = ResultBatcher::new(app, sid.clone());
        let _ = match search_mode {
            SearchMode::FileName => {
                search_filenames(&project_root, &query, use_regex, &cancel, &mut batcher)
            }
            SearchMode::FileContent => {
                search_contents(&project_root, &query, use_regex, &cancel, &mut batcher)
            }
        };
        let cancelled = cancel.load(Ordering::Relaxed);
        batcher.finish(cancelled);
        manager.remove(&sid);
    });

    Ok(())
}

#[tauri::command]
pub fn cancel_search(state: tauri::State<'_, SearchManager>, search_id: String) {
    state.cancel(&search_id);
}
```

- [ ] **Step 2: Register search module and commands in lib.rs**

In `src-tauri/src/lib.rs`:

Add after line 5 (`mod fs;`):
```rust
mod search;
```

Add after line 29 (`.manage(fs::FsWatcherManager::new())`):
```rust
        .manage(search::SearchManager::new())
```

Add inside `invoke_handler![]`, after `clipboard::save_clipboard_text,` (line 94):
```rust
            search::start_search,
            search::cancel_search,
```

- [ ] **Step 3: Verify Rust compilation**

Run: `cd src-tauri && cargo build 2>&1 | tail -5`

Expected: `Finished` without errors.

- [ ] **Step 4: Run all Rust tests**

Run: `cd src-tauri && cargo test 2>&1 | tail -20`

Expected: All tests pass including the new search tests.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/search.rs src-tauri/src/lib.rs
git commit -m "feat(search): 搜索逻辑和 Tauri 命令

- ResultBatcher 批量推送搜索结果（每 50 条或每 100ms）
- search_filenames: WalkBuilder 遍历 + 文件名子串/正则匹配
- search_contents: 遍历文件 + 跳过二进制 + 逐行匹配，返回行号和匹配位置
- start_search 命令：验证参数后 spawn 线程异步执行，立即返回
- cancel_search 命令：设置取消标记，搜索线程检查后退出
- lib.rs 注册 search 模块、SearchManager 状态和两个命令"
```

---

### Task 3: Frontend — TypeScript types and store

**Files:**
- Modify: `src/types.ts:136+`
- Modify: `src/store.ts:270-312,314-340`

- [ ] **Step 1: Add search types to types.ts**

Insert after the `FsChangePayload` interface (after line 157 in `src/types.ts`):

```typescript

// === 搜索 ===

export interface SearchResultItem {
  filePath: string;
  fileName: string;
  lineNumber?: number;
  lineContent?: string;
  matchRanges: [number, number][];
}

export interface SearchResultsPayload {
  searchId: string;
  items: SearchResultItem[];
}

export interface SearchCompletePayload {
  searchId: string;
  totalCount: number;
  cancelled: boolean;
}
```

- [ ] **Step 2: Add searchModalOpen to the store interface**

In `src/store.ts`, add to the `AppStore` interface (before the closing `}` around line 312):

```typescript
  // 搜索弹窗
  searchModalOpen: boolean;
  setSearchModalOpen: (open: boolean) => void;
```

- [ ] **Step 3: Add searchModalOpen initial value and setter**

In `src/store.ts`, inside `create<AppStore>((set, get) => ({`, add after `markersByPty: new Map(),` (around line 340):

```typescript
  searchModalOpen: false,
  setSearchModalOpen: (open) => set({ searchModalOpen: open }),
```

- [ ] **Step 4: Verify TypeScript compilation**

Run: `cd /d/Git/mini-term/.worktrees/feature-file-search && npx tsc --noEmit 2>&1 | head -20`

Expected: No type errors (or only pre-existing ones).

- [ ] **Step 5: Commit**

```bash
git add src/types.ts src/store.ts
git commit -m "feat(search): 前端搜索类型定义和 store 状态

- types.ts 新增 SearchResultItem、SearchResultsPayload、SearchCompletePayload
- store 新增 searchModalOpen 布尔值和 setter，控制搜索弹窗显隐"
```

---

### Task 4: SearchModal 组件

**Files:**
- Create: `src/components/SearchModal.tsx`

- [ ] **Step 1: Create SearchModal.tsx with complete implementation**

Create `src/components/SearchModal.tsx`:

```tsx
import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore } from '../store';
import { useTauriEvent } from '../hooks/useTauriEvent';
import { FileViewerModal } from './FileViewerModal';
import type { SearchResultItem, SearchResultsPayload, SearchCompletePayload } from '../types';

// ── Keyword highlight helper ──

function HighlightText({ text, ranges }: { text: string; ranges: [number, number][] }) {
  if (ranges.length === 0) return <>{text}</>;
  const parts: React.ReactNode[] = [];
  let lastEnd = 0;
  for (const [start, end] of ranges) {
    if (start > lastEnd) {
      parts.push(text.slice(lastEnd, start));
    }
    parts.push(
      <span key={start} className="bg-[var(--color-warning)]/30 text-[var(--color-warning)] rounded-sm px-[1px]">
        {text.slice(start, end)}
      </span>,
    );
    lastEnd = end;
  }
  if (lastEnd < text.length) {
    parts.push(text.slice(lastEnd));
  }
  return <>{parts}</>;
}

// ── Content results grouped by file ──

function ContentResults({
  results,
  onResultClick,
  onResultDoubleClick,
}: {
  results: SearchResultItem[];
  onResultClick: (item: SearchResultItem) => void;
  onResultDoubleClick: (item: SearchResultItem) => void;
}) {
  const grouped = new Map<string, SearchResultItem[]>();
  for (const item of results) {
    const group = grouped.get(item.filePath) ?? [];
    group.push(item);
    grouped.set(item.filePath, group);
  }

  return (
    <>
      {Array.from(grouped.entries()).map(([filePath, items]) => (
        <div key={filePath}>
          <div className="px-4 py-1.5 text-xs text-[var(--accent)] bg-[var(--bg-elevated)] font-medium sticky top-0 z-10 flex items-center gap-2">
            <span>{items[0].fileName}</span>
            <span className="text-[var(--text-muted)] truncate">{filePath}</span>
            <span className="text-[var(--text-muted)]">({items.length})</span>
          </div>
          {items.map((item, i) => (
            <div
              key={i}
              className="flex items-center gap-2 px-4 py-1 cursor-pointer hover:bg-[var(--border-subtle)] transition-colors font-mono text-xs"
              onClick={() => onResultClick(item)}
              onDoubleClick={() => onResultDoubleClick(item)}
            >
              <span className="w-10 text-right text-[var(--text-muted)] flex-shrink-0 select-none">
                {item.lineNumber}
              </span>
              <span className="text-[var(--text-primary)] truncate">
                <HighlightText text={item.lineContent ?? ''} ranges={item.matchRanges} />
              </span>
            </div>
          ))}
        </div>
      ))}
    </>
  );
}

// ── SearchModal ──

interface SearchModalProps {
  open: boolean;
  onClose: () => void;
}

export function SearchModal({ open, onClose }: SearchModalProps) {
  const [query, setQuery] = useState('');
  const [mode, setMode] = useState<'filename' | 'content'>('filename');
  const [useRegex, setUseRegex] = useState(false);
  const [searchId, setSearchId] = useState<string | null>(null);
  const [results, setResults] = useState<SearchResultItem[]>([]);
  const [status, setStatus] = useState<'idle' | 'searching' | 'done'>('idle');
  const [totalCount, setTotalCount] = useState(0);
  const [viewFilePath, setViewFilePath] = useState<string | null>(null);
  const [viewHighlightLine, setViewHighlightLine] = useState<number | undefined>();
  const inputRef = useRef<HTMLInputElement>(null);
  const searchIdRef = useRef<string | null>(null);

  const activeProjectId = useAppStore((s) => s.activeProjectId);
  const config = useAppStore((s) => s.config);
  const project = config.projects.find((p) => p.id === activeProjectId);

  // Keep ref in sync so event listeners always see the latest value
  searchIdRef.current = searchId;

  // Focus input when modal opens; reset state on close
  useEffect(() => {
    if (open) {
      setTimeout(() => inputRef.current?.focus(), 50);
    } else {
      if (searchIdRef.current) {
        invoke('cancel_search', { searchId: searchIdRef.current }).catch(() => {});
      }
      setQuery('');
      setResults([]);
      setStatus('idle');
      setTotalCount(0);
      setSearchId(null);
    }
  }, [open]);

  // Escape to close (only when FileViewerModal is not open)
  useEffect(() => {
    if (!open) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !viewFilePath) {
        onClose();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [open, onClose, viewFilePath]);

  // Listen for search results
  useTauriEvent<SearchResultsPayload>(
    'search-results',
    useCallback((payload) => {
      if (payload.searchId !== searchIdRef.current) return;
      setResults((prev) => {
        if (prev.length >= 1000) return prev;
        const remaining = 1000 - prev.length;
        return [...prev, ...payload.items.slice(0, remaining)];
      });
    }, []),
  );

  // Listen for search complete
  useTauriEvent<SearchCompletePayload>(
    'search-complete',
    useCallback((payload) => {
      if (payload.searchId !== searchIdRef.current) return;
      setStatus('done');
      setTotalCount(payload.totalCount);
    }, []),
  );

  const handleSearch = useCallback(() => {
    if (!query.trim() || !project) return;
    if (searchIdRef.current) {
      invoke('cancel_search', { searchId: searchIdRef.current }).catch(() => {});
    }
    const newId = crypto.randomUUID();
    searchIdRef.current = newId;
    setSearchId(newId);
    setResults([]);
    setStatus('searching');
    setTotalCount(0);
    invoke('start_search', {
      projectRoot: project.path,
      query: query.trim(),
      mode,
      useRegex,
      searchId: newId,
    }).catch(() => setStatus('done'));
  }, [query, project, mode, useRegex]);

  const handleResultClick = useCallback(
    (item: SearchResultItem) => {
      if (!project) return;
      const sep = project.path.includes('\\') ? '\\' : '/';
      setViewFilePath(project.path + sep + item.filePath);
      setViewHighlightLine(item.lineNumber ?? undefined);
    },
    [project],
  );

  const handleResultDoubleClick = useCallback(
    (item: SearchResultItem) => {
      if (!project) return;
      const sep = project.path.includes('\\') ? '\\' : '/';
      invoke('open_in_editor', {
        path: project.path + sep + item.filePath,
      }).catch(() => {});
    },
    [project],
  );

  if (!open) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center select-text" onClick={onClose}>
      <div className="absolute inset-0 bg-black/50 backdrop-blur-sm" />
      <div
        className="relative flex flex-col overflow-hidden bg-[var(--bg-surface)] border border-[var(--border-strong)] rounded-[var(--radius-md)] shadow-[var(--shadow-overlay)] animate-slide-in"
        style={{ width: '80vw', height: '70vh', maxWidth: '900px' }}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--border-subtle)] flex-shrink-0">
          <div className="flex items-center gap-3">
            <span className="text-base font-medium text-[var(--accent)]">搜索</span>
            <div className="flex rounded-[var(--radius-sm)] border border-[var(--border-default)] overflow-hidden text-xs">
              <button
                className={`px-2.5 py-1 transition-colors ${mode === 'filename' ? 'bg-[var(--accent)] text-[var(--bg-base)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                onClick={() => setMode('filename')}
              >
                文件名
              </button>
              <button
                className={`px-2.5 py-1 transition-colors ${mode === 'content' ? 'bg-[var(--accent)] text-[var(--bg-base)]' : 'text-[var(--text-muted)] hover:text-[var(--text-primary)]'}`}
                onClick={() => setMode('content')}
              >
                内容
              </button>
            </div>
          </div>
          <button
            className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-lg leading-none"
            onClick={onClose}
          >
            ✕
          </button>
        </div>

        {/* Search input */}
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--border-subtle)]">
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') handleSearch();
            }}
            placeholder={mode === 'filename' ? '搜索文件名...' : '搜索文件内容...'}
            className="flex-1 bg-[var(--bg-base)] border border-[var(--border-default)] rounded-[var(--radius-sm)] px-3 py-1.5 text-sm text-[var(--text-primary)] placeholder:text-[var(--text-muted)] focus:outline-none focus:border-[var(--accent)]"
          />
          <button
            className={`px-2 py-1.5 text-xs rounded-[var(--radius-sm)] border transition-colors font-mono ${
              useRegex
                ? 'bg-[var(--accent)] text-[var(--bg-base)] border-[var(--accent)]'
                : 'text-[var(--text-muted)] border-[var(--border-default)] hover:text-[var(--text-primary)]'
            }`}
            onClick={() => setUseRegex(!useRegex)}
            title="正则表达式"
          >
            .*
          </button>
          <button
            onClick={handleSearch}
            disabled={!query.trim() || status === 'searching'}
            className="px-3 py-1.5 text-xs rounded-[var(--radius-sm)] bg-[var(--accent)] text-[var(--bg-base)] hover:opacity-90 disabled:opacity-50 transition-opacity"
          >
            搜索
          </button>
        </div>

        {/* Results */}
        <div className="flex-1 overflow-auto bg-[var(--bg-base)]">
          {status === 'searching' && results.length === 0 && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)]">搜索中...</div>
          )}
          {status === 'idle' && (
            <div className="flex items-center justify-center h-full text-[var(--text-muted)] text-sm">
              输入关键词后按 Enter 开始搜索
            </div>
          )}
          {results.length > 0 && (
            <div className="divide-y divide-[var(--border-subtle)]">
              {mode === 'filename'
                ? results.map((item, i) => (
                    <div
                      key={i}
                      className="flex items-center gap-2 px-4 py-1.5 cursor-pointer hover:bg-[var(--border-subtle)] transition-colors"
                      onClick={() => handleResultClick(item)}
                      onDoubleClick={() => handleResultDoubleClick(item)}
                    >
                      <span className="text-sm text-[var(--text-primary)]">
                        <HighlightText text={item.fileName} ranges={item.matchRanges} />
                      </span>
                      <span className="text-xs text-[var(--text-muted)] truncate">{item.filePath}</span>
                    </div>
                  ))
                : (
                    <ContentResults results={results} onResultClick={handleResultClick} onResultDoubleClick={handleResultDoubleClick} />
                  )}
            </div>
          )}
          {results.length >= 1000 && (
            <div className="px-4 py-2 text-xs text-[var(--color-warning)] bg-[var(--bg-elevated)]">
              已显示前 1000 条结果，更多结果未显示
            </div>
          )}
        </div>

        {/* Status bar */}
        <div className="flex items-center px-4 py-1.5 border-t border-[var(--border-subtle)] text-xs text-[var(--text-muted)] flex-shrink-0">
          {status === 'searching' && <span>搜索中... 已找到 {results.length} 条</span>}
          {status === 'done' && mode === 'filename' && <span>找到 {totalCount} 个文件</span>}
          {status === 'done' && mode === 'content' && <span>找到 {totalCount} 处匹配</span>}
          {status === 'idle' && <span>Ctrl+Shift+F 打开搜索</span>}
        </div>
      </div>

      {viewFilePath && project && (
        <div onClick={(e) => e.stopPropagation()}>
          <FileViewerModal
            open={!!viewFilePath}
            onClose={() => setViewFilePath(null)}
            filePath={viewFilePath}
            projectRoot={project.path}
            highlightLine={viewHighlightLine}
          />
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 2: Verify TypeScript compilation**

Run: `cd /d/Git/mini-term/.worktrees/feature-file-search && npx tsc --noEmit 2>&1 | head -20`

Expected: Type errors only for `highlightLine` prop on FileViewerModal (not yet added — that's Task 5).

- [ ] **Step 3: Commit**

```bash
git add src/components/SearchModal.tsx
git commit -m "feat(search): SearchModal 组件

- 顶部模式切换（文件名/内容）、正则开关、搜索输入框
- 监听 search-results / search-complete 事件，逐批渲染结果
- 文件名模式平铺显示，内容模式按文件分组 + 行号
- HighlightText 组件实现关键词高亮
- 单击打开 FileViewerModal 预览，双击用编辑器打开
- 结果上限 1000 条，底部状态栏显示匹配统计
- Escape 关闭弹窗，关闭时自动取消进行中搜索"
```

---

### Task 5: FileViewerModal — 高亮行支持

**Files:**
- Modify: `src/components/FileViewerModal.tsx:7-12,50-55,133-144`

- [ ] **Step 1: Add highlightLine prop to FileViewerModal**

In `src/components/FileViewerModal.tsx`, modify the props interface (line 7-12):

```typescript
interface FileViewerModalProps {
  open: boolean;
  onClose: () => void;
  filePath: string;
  projectRoot: string;
  highlightLine?: number;
}
```

Update the function signature (line 18):

```typescript
export function FileViewerModal({ open, onClose, filePath, projectRoot, highlightLine }: FileViewerModalProps) {
```

- [ ] **Step 2: Add scroll-to-line logic and highlight styling**

Add a ref after the existing state declarations (after line 23, `const [preview, setPreview] = useState(true);`):

```typescript
  const highlightRef = useRef<HTMLDivElement>(null);
```

Add `useRef` to the React import on line 1 (it is NOT currently imported). Change:

```typescript
import { useState, useEffect, useMemo } from 'react';
```

to:

```typescript
import { useState, useEffect, useMemo, useRef } from 'react';
```

Add a useEffect to scroll to the highlighted line (after the Escape key useEffect, around line 44):

```typescript
  useEffect(() => {
    if (result && highlightLine && highlightRef.current) {
      highlightRef.current.scrollIntoView({ block: 'center', behavior: 'smooth' });
    }
  }, [result, highlightLine]);
```

- [ ] **Step 3: Apply highlight styling to the target line**

Modify the line rendering block (around lines 133-144). Change:

```tsx
              {result.content.split('\n').map((line, i) => (
                <div key={i} className="flex hover:bg-[var(--border-subtle)]">
                  <span className="w-12 text-right pr-3 text-[var(--text-muted)] select-none flex-shrink-0 opacity-40">
                    {i + 1}
                  </span>
                  <span className="flex-1 whitespace-pre px-2 text-[var(--text-primary)]">
                    {line}
                  </span>
                </div>
              ))}
```

to:

```tsx
              {result.content.split('\n').map((line, i) => (
                <div
                  key={i}
                  ref={i + 1 === highlightLine ? highlightRef : undefined}
                  className={`flex hover:bg-[var(--border-subtle)] ${i + 1 === highlightLine ? 'bg-[var(--accent-muted)]' : ''}`}
                >
                  <span className="w-12 text-right pr-3 text-[var(--text-muted)] select-none flex-shrink-0 opacity-40">
                    {i + 1}
                  </span>
                  <span className="flex-1 whitespace-pre px-2 text-[var(--text-primary)]">
                    {line}
                  </span>
                </div>
              ))}
```

- [ ] **Step 4: Verify TypeScript compilation**

Run: `cd /d/Git/mini-term/.worktrees/feature-file-search && npx tsc --noEmit 2>&1 | head -20`

Expected: No type errors.

- [ ] **Step 5: Commit**

```bash
git add src/components/FileViewerModal.tsx
git commit -m "feat(search): FileViewerModal 支持高亮行

- 新增 highlightLine 可选 prop
- 打开后自动滚动到目标行（smooth + center）
- 目标行添加 accent-muted 背景高亮"
```

---

### Task 6: Integration — App.tsx 快捷键 + FileTree 搜索按钮

**Files:**
- Modify: `src/App.tsx:1-17,25-28,191-285`
- Modify: `src/components/FileTree.tsx:417-428`

- [ ] **Step 1: Import SearchModal and store state in App.tsx**

In `src/App.tsx`, add import after line 15 (`import { SettingsModal } from './components/SettingsModal';`):

```typescript
import { SearchModal } from './components/SearchModal';
```

- [ ] **Step 2: Add searchModalOpen state and Ctrl+Shift+F handler in App.tsx**

Inside the `App()` component, after the existing state declarations (around line 33, after `const updatePaneStatusByPty = ...`), add:

```typescript
  const searchModalOpen = useAppStore((s) => s.searchModalOpen);
  const setSearchModalOpen = useAppStore((s) => s.setSearchModalOpen);
```

Add a useEffect for the keyboard shortcut (after the drag prevention useEffect, around line 96):

```typescript
  // Ctrl+Shift+F 打开/关闭搜索弹窗
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.shiftKey && e.key === 'F') {
        e.preventDefault();
        const { searchModalOpen: isOpen, setSearchModalOpen: setOpen } = useAppStore.getState();
        setOpen(!isOpen);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);
```

- [ ] **Step 3: Mount SearchModal in App.tsx JSX**

After the `<SettingsModal>` line (line 283), add:

```tsx
      <SearchModal open={searchModalOpen} onClose={() => setSearchModalOpen(false)} />
```

- [ ] **Step 4: Add search button to FileTree toolbar**

In `src/components/FileTree.tsx`, import the store setter. The `useAppStore` import already exists (line 6). Add after the existing store reads (around the toolbar area, in the `FileTree` component body):

```typescript
  const setSearchModalOpen = useAppStore((s) => s.setSearchModalOpen);
```

In the toolbar buttons area (around line 417-428), add a search button BEFORE the refresh button:

```tsx
          <button
            type="button"
            onClick={() => setSearchModalOpen(true)}
            title="搜索文件 (Ctrl+Shift+F)"
            className="text-[var(--text-muted)] hover:text-[var(--text-primary)] transition-colors text-sm leading-none px-1.5 py-0.5 rounded-[var(--radius-sm)] hover:bg-[var(--border-subtle)]"
          >
            ⌕
          </button>
```

- [ ] **Step 5: Verify TypeScript compilation**

Run: `cd /d/Git/mini-term/.worktrees/feature-file-search && npx tsc --noEmit 2>&1 | head -20`

Expected: No type errors.

- [ ] **Step 6: Commit**

```bash
git add src/App.tsx src/components/FileTree.tsx
git commit -m "feat(search): 集成搜索入口

- App.tsx 挂载 SearchModal，注册 Ctrl+Shift+F 全局快捷键
- FileTree 工具栏新增搜索按钮（⌕），点击打开搜索弹窗
- 弹窗显隐通过 store.searchModalOpen 控制"
```

---

### Task 7: 端到端验证

**Files:** None (testing only)

- [ ] **Step 1: Start dev environment**

Run: `cd /d/Git/mini-term/.worktrees/feature-file-search && npm run tauri dev`

- [ ] **Step 2: Verify search entry points**

- Press `Ctrl+Shift+F` → SearchModal should open
- Press `Ctrl+Shift+F` again → SearchModal should close
- Click the ⌕ button in FileTree toolbar → SearchModal should open
- Press `Escape` → SearchModal should close

- [ ] **Step 3: Verify filename search**

- Open SearchModal, mode = "文件名"
- Type `App` → press Enter
- Results should show `App.tsx` with "App" highlighted
- Click a result → FileViewerModal opens

- [ ] **Step 4: Verify content search**

- Switch to "内容" mode
- Type `useState` → press Enter
- Results should appear grouped by file, with line numbers and highlighted matches
- Click a result → FileViewerModal opens, scrolls to the matching line with highlight

- [ ] **Step 5: Verify regex mode**

- Enable `.*` regex toggle
- Type `use(State|Effect)` → press Enter
- Results should match both `useState` and `useEffect`

- [ ] **Step 6: Verify cancel and re-search**

- Type a query and press Enter (while searching)
- Change query and press Enter again before first search completes
- Results should switch to the new query cleanly

- [ ] **Step 7: Verify edge cases**

- Search with empty query → should not trigger
- Search in content mode for something with many matches → status bar shows total count, results capped at 1000
- Close modal during active search → search should be cancelled
