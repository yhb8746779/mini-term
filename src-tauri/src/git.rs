use git2::{Repository, Status, StatusOptions};
use pathdiff::diff_paths;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub enum GitStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitFileStatus {
    pub path: String,
    pub old_path: Option<String>,
    pub status: GitStatus,
    pub status_label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: String,
    pub content: String,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GitDiffResult {
    pub old_content: String,
    pub new_content: String,
    pub hunks: Vec<DiffHunk>,
    pub is_binary: bool,
    pub too_large: bool,
}

// ---------------------------------------------------------------------------
// Task 2: get_git_status implementation
// ---------------------------------------------------------------------------

fn map_status(status: Status, is_empty_repo: bool) -> Option<GitStatus> {
    if status.contains(Status::CONFLICTED) {
        return Some(GitStatus::Conflicted);
    }
    if status.contains(Status::INDEX_RENAMED) || status.contains(Status::WT_RENAMED) {
        return Some(GitStatus::Renamed);
    }
    if status.contains(Status::INDEX_NEW) {
        return Some(GitStatus::Added);
    }
    if status.contains(Status::INDEX_MODIFIED) || status.contains(Status::WT_MODIFIED) {
        return Some(GitStatus::Modified);
    }
    if status.contains(Status::INDEX_DELETED) || status.contains(Status::WT_DELETED) {
        return Some(GitStatus::Deleted);
    }
    if status.contains(Status::WT_NEW) {
        if is_empty_repo {
            return Some(GitStatus::Added);
        } else {
            return Some(GitStatus::Untracked);
        }
    }
    None
}

fn status_label(status: &GitStatus) -> &'static str {
    match status {
        GitStatus::Modified => "M",
        GitStatus::Added => "A",
        GitStatus::Deleted => "D",
        GitStatus::Renamed => "R",
        GitStatus::Untracked => "?",
        GitStatus::Conflicted => "C",
    }
}

fn collect_repo_status(
    repo: &Repository,
    path_prefix: Option<&Path>,
) -> Result<Vec<GitFileStatus>, String> {
    let is_empty_repo = repo.head().is_err();

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);

    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for entry in statuses.iter() {
        let raw_path = entry.path().unwrap_or("").to_string();
        let s = entry.status();

        let git_status = match map_status(s, is_empty_repo) {
            Some(gs) => gs,
            None => continue,
        };

        let label = status_label(&git_status).to_string();

        // Compute path relative to path_prefix (if given), else use raw_path
        let display_path = if let Some(prefix) = path_prefix {
            let repo_workdir = repo.workdir().unwrap_or_else(|| repo.path());
            let abs = repo_workdir.join(&raw_path);
            diff_paths(&abs, prefix)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| raw_path.clone())
        } else {
            raw_path.clone()
        };

        // old_path for renames
        let old_path = if matches!(git_status, GitStatus::Renamed) {
            entry.head_to_index().and_then(|d| {
                d.old_file()
                    .path()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
        } else {
            None
        };

        result.push(GitFileStatus {
            path: display_path,
            old_path,
            status: git_status,
            status_label: label,
        });
    }

    Ok(result)
}

#[tauri::command]
pub fn get_git_status(project_path: String) -> Result<Vec<GitFileStatus>, String> {
    let path = Path::new(&project_path);

    // Try to discover a repo at or above project_path
    if let Ok(repo) = Repository::discover(path) {
        return collect_repo_status(&repo, Some(path));
    }

    // Fall back: scan one level of subdirectories
    let mut all = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let sub = entry.path();
            if sub.is_dir() {
                if let Ok(repo) = Repository::discover(&sub) {
                    // Only use if the workdir is exactly this subdirectory (avoid climbing up)
                    if let Some(workdir) = repo.workdir() {
                        if workdir.canonicalize().ok() == sub.canonicalize().ok() {
                            if let Ok(mut files) = collect_repo_status(&repo, Some(path)) {
                                all.append(&mut files);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(all)
}

// ---------------------------------------------------------------------------
// Task 3: get_git_diff implementation
// ---------------------------------------------------------------------------

fn get_head_content(repo: &Repository, rel_path: &str) -> Result<Option<String>, String> {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return Ok(None), // empty repo
    };
    let tree = head
        .peel_to_tree()
        .map_err(|e| e.to_string())?;
    let entry = match tree.get_path(Path::new(rel_path)) {
        Ok(e) => e,
        Err(_) => return Ok(Some(String::new())), // file not yet in HEAD
    };
    let obj = entry.to_object(repo).map_err(|e| e.to_string())?;
    let blob = obj.as_blob().ok_or("not a blob")?;

    if blob.is_binary() {
        return Err("binary".to_string());
    }
    let content = std::str::from_utf8(blob.content())
        .map_err(|_| "binary".to_string())?
        .to_string();
    Ok(Some(content))
}

// LCS-based diff producing DiffHunks (context = 3 lines)
fn build_hunks(old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffHunk> {
    let m = old_lines.len();
    let n = new_lines.len();

    // LCS DP table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            if old_lines[i] == new_lines[j] {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }

    // Produce flat edit list: ('=', old_i, new_j) | ('-', old_i, _) | ('+', _, new_j)
    let mut flat: Vec<(char, usize, usize)> = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < m || j < n {
        if i < m && j < n && old_lines[i] == new_lines[j] {
            flat.push(('=', i, j));
            i += 1;
            j += 1;
        } else if j < n && (i >= m || dp[i][j + 1] >= dp[i + 1][j]) {
            flat.push(('+', i, j));
            j += 1;
        } else {
            flat.push(('-', i, j));
            i += 1;
        }
    }

    // Group into hunks (context = 3 lines)
    const CONTEXT: usize = 3;
    let mut hunks: Vec<DiffHunk> = Vec::new();

    // Find ranges of non-equal edits, expand with context
    let changed_indices: Vec<usize> = flat
        .iter()
        .enumerate()
        .filter(|(_, (k, _, _))| *k != '=')
        .map(|(idx, _)| idx)
        .collect();

    if changed_indices.is_empty() {
        return hunks;
    }

    // Group changed indices into contiguous ranges (with context)
    let mut groups: Vec<(usize, usize)> = Vec::new(); // (start, end) in flat[]
    let start = changed_indices[0].saturating_sub(CONTEXT);
    let end = (changed_indices[0] + CONTEXT + 1).min(flat.len());
    groups.push((start, end));

    for &idx in &changed_indices[1..] {
        let last = groups.last_mut().unwrap();
        let expanded_start = idx.saturating_sub(CONTEXT);
        let expanded_end = (idx + CONTEXT + 1).min(flat.len());
        if expanded_start <= last.1 {
            last.1 = last.1.max(expanded_end);
        } else {
            groups.push((expanded_start, expanded_end));
        }
    }

    for (grp_start, grp_end) in groups {
        let slice = &flat[grp_start..grp_end];
        let mut lines_out: Vec<DiffLine> = Vec::new();
        let mut old_start = 0u32;
        let mut new_start = 0u32;
        let mut old_count = 0u32;
        let mut new_count = 0u32;
        let mut first = true;

        for (k, oi, ni) in slice {
            let old_lineno = (*oi as u32) + 1;
            let new_lineno = (*ni as u32) + 1;
            match k {
                '=' => {
                    if first {
                        old_start = old_lineno;
                        new_start = new_lineno;
                        first = false;
                    }
                    lines_out.push(DiffLine {
                        kind: "context".to_string(),
                        content: old_lines[*oi].to_string(),
                        old_lineno: Some(old_lineno),
                        new_lineno: Some(new_lineno),
                    });
                    old_count += 1;
                    new_count += 1;
                }
                '-' => {
                    if first {
                        old_start = old_lineno;
                        // new_start might be the next insert; approximate
                        new_start = (*ni as u32) + 1;
                        first = false;
                    }
                    lines_out.push(DiffLine {
                        kind: "delete".to_string(),
                        content: old_lines[*oi].to_string(),
                        old_lineno: Some(old_lineno),
                        new_lineno: None,
                    });
                    old_count += 1;
                }
                '+' => {
                    if first {
                        old_start = (*oi as u32) + 1;
                        new_start = new_lineno;
                        first = false;
                    }
                    lines_out.push(DiffLine {
                        kind: "add".to_string(),
                        content: new_lines[*ni].to_string(),
                        old_lineno: None,
                        new_lineno: Some(new_lineno),
                    });
                    new_count += 1;
                }
                _ => {}
            }
        }

        hunks.push(DiffHunk {
            old_start,
            old_lines: old_count,
            new_start,
            new_lines: new_count,
            lines: lines_out,
        });
    }

    hunks
}

fn full_replace_diff(old_content: &str, new_content: &str) -> Vec<DiffHunk> {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let mut lines_out: Vec<DiffLine> = Vec::new();

    for (i, l) in old_lines.iter().enumerate() {
        lines_out.push(DiffLine {
            kind: "delete".to_string(),
            content: l.to_string(),
            old_lineno: Some((i as u32) + 1),
            new_lineno: None,
        });
    }
    for (i, l) in new_lines.iter().enumerate() {
        lines_out.push(DiffLine {
            kind: "add".to_string(),
            content: l.to_string(),
            old_lineno: None,
            new_lineno: Some((i as u32) + 1),
        });
    }

    if lines_out.is_empty() {
        return Vec::new();
    }

    vec![DiffHunk {
        old_start: 1,
        old_lines: old_lines.len() as u32,
        new_start: 1,
        new_lines: new_lines.len() as u32,
        lines: lines_out,
    }]
}

#[tauri::command]
pub fn get_git_diff(project_path: String, file_path: String) -> Result<GitDiffResult, String> {
    let project = Path::new(&project_path);
    let abs_file = project.join(&file_path);

    let repo = Repository::discover(&abs_file).map_err(|e| e.to_string())?;
    let workdir = repo
        .workdir()
        .ok_or("bare repository not supported")?;

    // Relative path inside repo
    let rel_path = diff_paths(&abs_file, workdir)
        .ok_or("file is outside repository working directory")?;
    let rel_str = rel_path.to_string_lossy().replace('\\', "/");

    // Read new (working tree) content
    let new_bytes = std::fs::read(&abs_file).map_err(|e| e.to_string())?;

    // Large file protection (> 1 MB)
    if new_bytes.len() > 1_048_576 {
        return Ok(GitDiffResult {
            old_content: String::new(),
            new_content: String::new(),
            hunks: Vec::new(),
            is_binary: false,
            too_large: true,
        });
    }

    // Binary detection
    let new_content = match std::str::from_utf8(&new_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            return Ok(GitDiffResult {
                old_content: String::new(),
                new_content: String::new(),
                hunks: Vec::new(),
                is_binary: true,
                too_large: false,
            })
        }
    };

    // Get HEAD content
    let old_content = match get_head_content(&repo, &rel_str)? {
        None => String::new(), // empty repo
        Some(s) => s,
    };

    // Check blob binary via git2 as well
    // (already covered by UTF-8 check above for new content; old content checked in get_head_content)

    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines_vec: Vec<&str> = new_content.lines().collect();

    let ol = old_lines.len() as u64;
    let nl = new_lines_vec.len() as u64;

    let hunks = if ol * nl > 10_000_000 {
        full_replace_diff(&old_content, &new_content)
    } else {
        build_hunks(&old_lines, &new_lines_vec)
    };

    Ok(GitDiffResult {
        old_content,
        new_content,
        hunks,
        is_binary: false,
        too_large: false,
    })
}
