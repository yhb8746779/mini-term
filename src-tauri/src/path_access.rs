use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};

use crate::config::ProjectConfig;

#[derive(Default)]
struct PathAccessState {
    projects: HashMap<String, Option<String>>,
    activated: HashSet<String>,
}

pub struct PathAccessManager {
    inner: Arc<Mutex<PathAccessState>>,
}

impl PathAccessManager {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PathAccessState::default())),
        }
    }
}

fn normalize_project_path(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    #[cfg(windows)]
    {
        let trimmed = path.trim_end_matches(['/', '\\']);
        if trimmed.len() == 2 && trimmed.ends_with(':') {
            format!("{trimmed}\\")
        } else if trimmed.is_empty() {
            path.to_string()
        } else {
            trimmed.to_string()
        }
    }

    #[cfg(not(windows))]
    {
        if path == "/" {
            "/".to_string()
        } else {
            path.trim_end_matches('/').to_string()
        }
    }
}

fn path_is_within(requested: &Path, root: &Path) -> bool {
    requested == root || requested.starts_with(root)
}

fn find_covering_project_root(state: &PathAccessState, requested_path: &str) -> Option<String> {
    let requested = Path::new(requested_path);

    state
        .projects
        .keys()
        .filter(|root| {
            let root_path = Path::new(root);
            path_is_within(requested, root_path)
        })
        .max_by_key(|root| root.len())
        .cloned()
}

pub fn sync_project_accesses(app: &AppHandle, projects: &[ProjectConfig]) {
    let state = app.state::<PathAccessManager>();
    let mut guard = state.inner.lock().unwrap();

    let next_projects: HashMap<String, Option<String>> = projects
        .iter()
        .map(|project| {
            (
                normalize_project_path(&project.path),
                project
                    .macos_bookmark
                    .as_ref()
                    .filter(|bookmark| !bookmark.is_empty())
                    .cloned(),
            )
        })
        .collect();

    guard.projects = next_projects;

    let valid_roots: HashSet<String> = projects
        .iter()
        .map(|project| normalize_project_path(&project.path))
        .collect();
    guard.activated.retain(|root| valid_roots.contains(root));
}

pub fn ensure_path_access(app: &AppHandle, requested_path: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        ensure_path_access_macos(app, requested_path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, requested_path);
        Ok(())
    }
}

#[tauri::command]
pub fn prepare_project_access(app: AppHandle, path: String) -> Result<(), String> {
    ensure_path_access(&app, &path)
}

#[cfg(target_os = "macos")]
fn ensure_path_access_macos(app: &AppHandle, requested_path: &str) -> Result<(), String> {
    let requested_norm = normalize_project_path(requested_path);
    let root = {
        let guard = app.state::<PathAccessManager>().inner.lock().unwrap();
        let root = if guard.projects.contains_key(&requested_norm) {
            Some(requested_norm.clone())
        } else {
            find_covering_project_root(&guard, &requested_norm)
        };

        let Some(root) = root else {
            return Ok(());
        };

        if guard.activated.contains(&root) {
            return Ok(());
        }

        root
    };

    let bookmark = {
        let guard = app.state::<PathAccessManager>().inner.lock().unwrap();
        guard.projects.get(&root).cloned().flatten()
    };

    let mut bookmark_to_store = None;

    if let Some(bookmark) = bookmark {
        match mac::activate_security_scoped_bookmark(&bookmark) {
            Ok(()) => {}
            Err(_) => {
                let regenerated = mac::create_security_scoped_bookmark(&root)?;
                mac::activate_security_scoped_bookmark(&regenerated)?;
                bookmark_to_store = Some(regenerated);
            }
        }
    } else {
        let generated = mac::create_security_scoped_bookmark(&root)?;
        mac::activate_security_scoped_bookmark(&generated)?;
        bookmark_to_store = Some(generated);
    }

    {
        let state = app.state::<PathAccessManager>();
        let mut guard = state.inner.lock().unwrap();
        guard.activated.insert(root.clone());
        if let Some(bookmark) = &bookmark_to_store {
            guard.projects.insert(root.clone(), Some(bookmark.clone()));
        }
    }

    if let Some(bookmark) = bookmark_to_store {
        crate::config::persist_project_bookmark(app, &root, bookmark)?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{c_char, c_void, CStr, CString};

    use base64::Engine;
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};

    const NSURL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE: usize = 1 << 11;
    const NSURL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE: usize = 1 << 10;

    fn nsstring(s: &str) -> Result<Retained<AnyObject>, String> {
        unsafe {
            let str_cls = AnyClass::get(c"NSString")
                .ok_or_else(|| "NSString class not found".to_string())?;
            let c_str = CString::new(s).map_err(|_| "path contains NUL byte".to_string())?;
            Ok(msg_send![str_cls, stringWithUTF8String: c_str.as_ptr()])
        }
    }

    fn file_url_from_path(path: &str) -> Result<Retained<AnyObject>, String> {
        unsafe {
            let url_cls = AnyClass::get(c"NSURL")
                .ok_or_else(|| "NSURL class not found".to_string())?;
            let ns_path = nsstring(path)?;
            let url_opt: Option<Retained<AnyObject>> = msg_send![url_cls, fileURLWithPath: &*ns_path];
            url_opt.ok_or_else(|| format!("无法创建文件 URL: {path}"))
        }
    }

    fn nsdata_from_bytes(bytes: &[u8]) -> Result<Retained<AnyObject>, String> {
        unsafe {
            let data_cls = AnyClass::get(c"NSData")
                .ok_or_else(|| "NSData class not found".to_string())?;
            Ok(msg_send![data_cls,
                dataWithBytes: bytes.as_ptr() as *const c_void
                length: bytes.len()
            ])
        }
    }

    fn data_to_vec(data: &AnyObject) -> Vec<u8> {
        unsafe {
            let length: usize = msg_send![data, length];
            let bytes: *const c_void = msg_send![data, bytes];
            if length == 0 || bytes.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(bytes as *const u8, length).to_vec()
            }
        }
    }

    fn ns_error_string(error: *mut AnyObject) -> String {
        if error.is_null() {
            return "unknown NSError".to_string();
        }

        unsafe {
            let desc: Retained<AnyObject> = msg_send![error, localizedDescription];
            let utf8: *const c_char = msg_send![&desc, UTF8String];
            if utf8.is_null() {
                "unknown NSError".to_string()
            } else {
                CStr::from_ptr(utf8).to_string_lossy().into_owned()
            }
        }
    }

    pub fn create_security_scoped_bookmark(path: &str) -> Result<String, String> {
        unsafe {
            let url = file_url_from_path(path)?;
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let data_opt: Option<Retained<AnyObject>> = msg_send![
                &url,
                bookmarkDataWithOptions: NSURL_BOOKMARK_CREATION_WITH_SECURITY_SCOPE
                includingResourceValuesForKeys: std::ptr::null::<AnyObject>()
                relativeToURL: std::ptr::null::<AnyObject>()
                error: &mut error
            ];

            let data = data_opt.ok_or_else(|| {
                format!(
                    "创建目录授权书签失败: {} ({})",
                    path,
                    ns_error_string(error)
                )
            })?;

            let bytes = data_to_vec(&data);
            Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
        }
    }

    pub fn activate_security_scoped_bookmark(bookmark: &str) -> Result<(), String> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(bookmark)
            .map_err(|e| format!("解析目录授权书签失败: {e}"))?;

        unsafe {
            let url_cls = AnyClass::get(c"NSURL")
                .ok_or_else(|| "NSURL class not found".to_string())?;
            let data = nsdata_from_bytes(&decoded)?;
            let mut stale: i8 = 0;
            let mut error: *mut AnyObject = std::ptr::null_mut();

            let url_opt: Option<Retained<AnyObject>> = msg_send![
                url_cls,
                URLByResolvingBookmarkData: &*data
                options: NSURL_BOOKMARK_RESOLUTION_WITH_SECURITY_SCOPE
                relativeToURL: std::ptr::null::<AnyObject>()
                bookmarkDataIsStale: &mut stale
                error: &mut error
            ];

            let url = url_opt.ok_or_else(|| {
                format!("恢复目录授权失败: {}", ns_error_string(error))
            })?;

            let started: i8 = msg_send![&url, startAccessingSecurityScopedResource];
            if started == 0 {
                return Err("startAccessingSecurityScopedResource 返回 NO".into());
            }

            // 让 access scope 持续到进程退出，避免 watcher / PTY / git 后续再次丢失访问权限。
            std::mem::forget(url);

            if stale != 0 {
                return Err("目录授权书签已过期".into());
            }

            Ok(())
        }
    }
}
