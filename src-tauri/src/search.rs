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

/// Convert byte-offset ranges to char-index ranges so the frontend HighlightText
/// (JS String.slice) works correctly on non-ASCII text (CJK, emoji, etc.).
fn byte_ranges_to_char_ranges(text: &str, byte_ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    if byte_ranges.is_empty() {
        return byte_ranges;
    }
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
