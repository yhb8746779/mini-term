use ignore::gitignore::Gitignore;
use notify::{RecommendedWatcher, RecursiveMode, Watcher, Event as NotifyEvent};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub ignored: bool,
}

fn build_gitignore(project_root: &Path) -> Option<Gitignore> {
    let gitignore_path = project_root.join(".gitignore");
    if !gitignore_path.exists() {
        return None;
    }
    let (gi, _err) = Gitignore::new(&gitignore_path);
    Some(gi)
}

pub const ALWAYS_IGNORE: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "__pycache__", ".superpowers"];

/// macOS 在 exFAT/FAT/网络卷上为每个文件生成的 AppleDouble 元数据（`._xxx`），
/// 内容是二进制，与真实文件同名易误点，文件树和搜索中一律隐藏。
pub fn is_apple_double(name: &str, is_dir: bool) -> bool {
    !is_dir && name.starts_with("._")
}

/// 同一路径的文件变更事件，150ms 内只向前端 emit 一次，防止驱动器根目录等高频场景卡死
const FS_EVENT_DEBOUNCE: Duration = Duration::from_millis(150);

#[cfg(test)]
fn should_ignore(name: &str, full_path: &Path, is_dir: bool, gitignore: &Option<Gitignore>) -> bool {
    if is_dir && ALWAYS_IGNORE.contains(&name) {
        return true;
    }
    if let Some(gi) = gitignore {
        return gi.matched(full_path, is_dir).is_ignore();
    }
    false
}

#[tauri::command]
pub fn list_directory(app: AppHandle, project_root: String, path: String) -> Result<Vec<FileEntry>, String> {
    let t0 = Instant::now();
    let result = crate::path_access::ensure_path_access(&app, &project_root)
        .and_then(|_| read_directory_entries(&project_root, &path));
    let outcome = match &result {
        Ok(entries) => format!("entry_count={}", entries.len()),
        Err(error) => format!("result=error | error={error}"),
    };
    crate::perf_log::log_perf(&app, "list_directory", &format!(
        "path={} | {} | cost_ms={}", path, outcome, t0.elapsed().as_millis()
    ));
    result
}

fn read_directory_entries(project_root: &str, path: &str) -> Result<Vec<FileEntry>, String> {
    let dir = Path::new(&path);
    let metadata = fs::metadata(dir).map_err(|e| format!("检查目录失败: {path}: {e}"))?;
    if !metadata.is_dir() {
        return Err(format!("Not a directory: {}", path));
    }
    let gitignore = build_gitignore(Path::new(&project_root));
    // Close the directory iterator before inspecting entries. Do not silently
    // turn IO/resource errors into an empty or incomplete file list.
    let directory_entries = fs::read_dir(dir)
        .map_err(|e| format!("读取目录失败: {path}: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("枚举目录失败: {path}: {e}"))?;
    let mut entries = Vec::new();
    for entry in directory_entries {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type()
            .map_err(|e| format!("读取文件类型失败: {}: {e}", entry.path().display()))?.is_dir();
        let full_path = entry.path();
        // ALWAYS_IGNORE 目录仍然完全隐藏
        if is_dir && ALWAYS_IGNORE.contains(&name.as_str()) {
            continue;
        }
        if is_apple_double(&name, is_dir) {
            continue;
        }
        let ignored = if let Some(gi) = &gitignore {
            gi.matched(&full_path, is_dir).is_ignore()
        } else {
            false
        };
        entries.push(FileEntry {
            name,
            path: full_path.to_string_lossy().to_string(),
            is_dir,
            ignored,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir)
            .then_with(|| a.ignored.cmp(&b.ignored))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FsChangePayload {
    project_path: String,
    path: String,
    kind: String,
}

pub struct FsWatcherManager {
    watchers: Arc<Mutex<HashMap<String, RecommendedWatcher>>>,
}

impl FsWatcherManager {
    pub fn new() -> Self {
        Self { watchers: Arc::new(Mutex::new(HashMap::new())) }
    }
}

#[tauri::command]
pub fn watch_directory(
    app: AppHandle,
    state: tauri::State<'_, FsWatcherManager>,
    path: String,
    project_path: String,
) -> Result<(), String> {
    crate::path_access::ensure_path_access(&app, &project_path)?;
    let watch_path = PathBuf::from(&path);
    let project_path_clone = project_path.clone();
    let app_clone = app.clone();

    // 每个路径独立防抖：150ms 内只向前端 emit 一次，避免驱动器根目录等高频场景卡死
    let last_emitted: Arc<Mutex<HashMap<String, Instant>>> = Arc::new(Mutex::new(HashMap::new()));

    let mut watcher = notify::recommended_watcher(move |res: Result<NotifyEvent, _>| {
        if let Ok(event) = res {
            let now = Instant::now();
            let mut last = last_emitted.lock().unwrap();
            for p in &event.paths {
                let path_str = p.to_string_lossy().to_string();
                let should_emit = last.get(&path_str)
                    .map_or(true, |t| now.duration_since(*t) >= FS_EVENT_DEBOUNCE);
                if should_emit {
                    last.insert(path_str.clone(), now);
                    let _ = app_clone.emit("fs-change", FsChangePayload {
                        project_path: project_path_clone.clone(),
                        path: path_str,
                        kind: format!("{:?}", event.kind),
                    });
                }
            }
        }
    }).map_err(|e| e.to_string())?;

    watcher.watch(&watch_path, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;

    let mut watchers = state.watchers.lock().unwrap();
    watchers.insert(path, watcher);
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContentResult {
    pub content: String,
    pub is_binary: bool,
    pub too_large: bool,
}

const MAX_FILE_VIEW_SIZE: u64 = 1_048_576; // 1MB

#[tauri::command]
pub fn read_file_content(app: AppHandle, path: String) -> Result<FileContentResult, String> {
    crate::path_access::ensure_path_access(&app, &path)?;
    let p = Path::new(&path);
    if !p.is_file() {
        return Err(format!("不是文件: {}", path));
    }
    let metadata = fs::metadata(p).map_err(|e| e.to_string())?;
    if metadata.len() > MAX_FILE_VIEW_SIZE {
        return Ok(FileContentResult { content: String::new(), is_binary: false, too_large: true });
    }
    let bytes = fs::read(p).map_err(|e| e.to_string())?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(FileContentResult { content: s, is_binary: false, too_large: false }),
        Err(_) => Ok(FileContentResult { content: String::new(), is_binary: true, too_large: false }),
    }
}

#[tauri::command]
pub fn create_file(app: AppHandle, path: String) -> Result<(), String> {
    crate::path_access::ensure_path_access(&app, &path)?;
    let p = Path::new(&path);
    if p.exists() {
        return Err(format!("已存在: {}", path));
    }
    fs::write(p, "").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_directory(app: AppHandle, path: String) -> Result<(), String> {
    crate::path_access::ensure_path_access(&app, &path)?;
    let p = Path::new(&path);
    if p.exists() {
        return Err(format!("已存在: {}", path));
    }
    fs::create_dir(p).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unwatch_directory(state: tauri::State<'_, FsWatcherManager>, path: String) -> Result<(), String> {
    let mut watchers = state.watchers.lock().unwrap();
    watchers.remove(&path);
    Ok(())
}

#[tauri::command]
pub fn rename_entry(app: AppHandle, old_path: String, new_name: String) -> Result<String, String> {
    crate::path_access::ensure_path_access(&app, &old_path)?;
    let p = Path::new(&old_path);
    if !p.exists() {
        return Err(format!("路径不存在: {}", old_path));
    }
    let parent = p.parent().ok_or("无法获取父目录")?;
    let new_path = parent.join(&new_name);
    if new_path.exists() {
        return Err(format!("目标已存在: {}", new_path.display()));
    }
    fs::rename(p, &new_path).map_err(|e| e.to_string())?;
    Ok(new_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn directory_resource_error_is_not_reported_as_empty() {
        let output = std::process::Command::new("/bin/sh")
            .arg("-c").arg("ulimit -Sn 64; exec \"$1\" --exact fs::tests::directory_pressure_child --nocapture")
            .arg("directory-test").arg(std::env::current_exe().unwrap())
            .env("MINITERM_DIRECTORY_PRESSURE_TEST", "1").output().unwrap();
        assert!(output.status.success(), "{} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }

    #[cfg(unix)]
    #[test]
    fn directory_pressure_child() {
        if std::env::var_os("MINITERM_DIRECTORY_PRESSURE_TEST").is_none() { return; }
        let directory = std::env::temp_dir().join(format!("mini-term-fs-pressure-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("present.txt"), "test").unwrap();
        let path = directory.to_str().unwrap();
        let mut held = Vec::new();
        while let Ok(file) = fs::File::open("/dev/null") { held.push(file); }
        let error = read_directory_entries(path, path).unwrap_err();
        assert!(error.contains("os error 24"), "{error}");
        drop(held);
        let entries = read_directory_entries(path, path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "present.txt");
        fs::remove_dir_all(directory).unwrap();
    }


    #[test]
    fn should_ignore_node_modules() {
        let path = Path::new("node_modules");
        assert!(should_ignore("node_modules", path, true, &None));
        let git_path = Path::new(".git");
        assert!(should_ignore(".git", git_path, true, &None));
    }

    #[test]
    fn should_not_ignore_src() {
        let path = Path::new("src");
        assert!(!should_ignore("src", path, true, &None));
    }
}
