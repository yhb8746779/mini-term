use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tauri::{AppHandle, Emitter};

/// 每个项目最多返回的会话数（避免拥有海量历史的项目卡死应用）
const MAX_SESSIONS_PER_PROJECT: usize = 50;

/// Codex 全局扫描文件上限（避免海量 session 目录遍历时间过长）
const MAX_CODEX_FILES_SCANNED: usize = 1000;

/// Session 结果缓存有效期：10 分钟内重复切换项目直接返回内存数据，不再扫文件
const CACHE_TTL: Duration = Duration::from_secs(600);

type CacheMap = Arc<Mutex<HashMap<String, (Vec<AiSession>, Instant)>>>;

/// 每个项目 session 列表的内存缓存（project_path → (sessions, fetched_at)）
/// inner 使用 Arc 以便后台刷新线程持有引用
pub struct SessionCache {
    inner: CacheMap,
}

impl SessionCache {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(HashMap::new())) }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSession {
    pub id: String,
    pub session_type: String, // "claude" | "codex"
    pub title: String,
    pub timestamp: String, // ISO 8601
}

/// 获取用户 home 目录
fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// 将项目路径编码为 Claude 项目目录名（`:` `\` `/` → `-`）
fn encode_project_path(project_path: &str) -> String {
    project_path
        .replace(':', "-")
        .replace('\\', "-")
        .replace('/', "-")
}

/// 路径统一化（小写 + 反斜杠，去尾部斜杠），用于 Windows 路径比较
fn normalize_path(path: &str) -> String {
    path.replace('/', "\\")
        .to_lowercase()
        .trim_end_matches('\\')
        .to_string()
}

// ─── Claude Sessions ───────────────────────────────────────────

fn get_claude_sessions(project_path: &str) -> Vec<AiSession> {
    let home = match home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    let encoded = encode_project_path(project_path);
    let sessions_dir = home.join(".claude").join("projects").join(&encoded);

    if !sessions_dir.exists() {
        return vec![];
    }

    let entries = match fs::read_dir(&sessions_dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    // 按修改时间排序，只读最近 MAX_SESSIONS_PER_PROJECT 个文件，避免海量历史卡死
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("jsonl") { return None; }
            let mtime = p.metadata().ok()?.modified().ok()?;
            Some((mtime, p))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0)); // 最新在前

    let mut sessions = Vec::new();
    for (_, path) in files.into_iter().take(MAX_SESSIONS_PER_PROJECT) {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let (title, timestamp) = read_claude_session_info(&path);

        sessions.push(AiSession {
            id,
            session_type: "claude".to_string(),
            title,
            timestamp,
        });
    }

    sessions
}

/// 读取 Claude JSONL，提取第一条 user message 的内容和时间戳
fn read_claude_session_info(path: &Path) -> (String, String) {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return ("Untitled".into(), String::new()),
    };

    let reader = BufReader::new(file);

    for line in reader.lines().take(50) {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let obj: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if obj.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }

        let content_val = obj.pointer("/message/content");

        let content = if let Some(s) = content_val.and_then(|c| c.as_str()) {
            s.to_string()
        } else if let Some(arr) = content_val.and_then(|c| c.as_array()) {
            // 多模态消息：取第一个 text block
            arr.iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        item.get("text").and_then(|t| t.as_str()).map(String::from)
                    } else {
                        None
                    }
                })
                .next()
                .unwrap_or_else(|| "Untitled".into())
        } else {
            "Untitled".into()
        };

        // 跳过系统注入消息（如 /clear 等本地命令产生的 <local-command-caveat> 等）
        let trimmed = content.trim_start();
        if trimmed.starts_with('<') {
            continue;
        }

        // 截断到 100 字符
        let title: String = content.chars().take(100).collect();

        let timestamp = obj
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        return (title, timestamp);
    }

    ("Untitled".into(), String::new())
}

// ─── Codex Sessions ────────────────────────────────────────────

fn get_codex_sessions(project_path: &str) -> Vec<AiSession> {
    let home = match home_dir() {
        Some(h) => h,
        None => return vec![],
    };

    let codex_dir = home.join(".codex");
    let sessions_dir = codex_dir.join("sessions");

    if !sessions_dir.exists() {
        return vec![];
    }

    // 加载 session_index.jsonl 中的 thread_name 映射
    let thread_names = load_codex_thread_names(&codex_dir);

    let mut sessions = Vec::new();
    let normalized_project = normalize_path(project_path);

    let mut scanned = 0usize;
    walk_codex_sessions(&sessions_dir, &normalized_project, &thread_names, &mut sessions, &mut scanned);

    sessions
}

/// 加载 Codex session_index.jsonl → { id: thread_name }
fn load_codex_thread_names(codex_dir: &Path) -> HashMap<String, String> {
    let index_path = codex_dir.join("session_index.jsonl");
    let mut map = HashMap::new();

    let file = match fs::File::open(&index_path) {
        Ok(f) => f,
        Err(_) => return map,
    };

    let reader = BufReader::new(file);
    for line in reader.lines().flatten() {
        if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&line) {
            if let (Some(id), Some(name)) = (
                obj.get("id").and_then(|v| v.as_str()),
                obj.get("thread_name").and_then(|v| v.as_str()),
            ) {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }

    map
}

/// 递归遍历 sessions/<year>/<month>/<day>/ 目录
/// scanned: 已扫描文件计数，超过 MAX_CODEX_FILES_SCANNED 时提前终止
fn walk_codex_sessions(
    dir: &Path,
    normalized_project: &str,
    thread_names: &HashMap<String, String>,
    sessions: &mut Vec<AiSession>,
    scanned: &mut usize,
) {
    if *scanned >= MAX_CODEX_FILES_SCANNED || sessions.len() >= MAX_SESSIONS_PER_PROJECT {
        return;
    }

    // 按名称倒序，使 2025 > 2024、12 > 11 等最新目录先处理，更快达到上限退出
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

    for entry in entries {
        if *scanned >= MAX_CODEX_FILES_SCANNED || sessions.len() >= MAX_SESSIONS_PER_PROJECT {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            walk_codex_sessions(&path, normalized_project, thread_names, sessions, scanned);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            *scanned += 1;
            if let Some(session) = try_read_codex_session(&path, normalized_project, thread_names) {
                sessions.push(session);
            }
        }
    }
}

/// 读取 Codex session 文件，匹配 cwd 后返回 AiSession
fn try_read_codex_session(
    path: &Path,
    normalized_project: &str,
    thread_names: &HashMap<String, String>,
) -> Option<AiSession> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut matched_id = None;
    let mut matched_timestamp = String::new();

    let mut lines_iter = reader.lines();

    // 第一遍：前 5 行找 session_meta，匹配 cwd
    for line in (&mut lines_iter).take(5) {
        let line = line.ok()?;
        let obj: serde_json::Value = serde_json::from_str(&line).ok()?;

        if obj.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
            continue;
        }

        let cwd = obj
            .pointer("/payload/cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if normalize_path(cwd) != normalized_project {
            return None;
        }

        matched_id = Some(
            obj.pointer("/payload/id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        );

        matched_timestamp = obj
            .pointer("/payload/timestamp")
            .or_else(|| obj.get("timestamp"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        break;
    }

    let id = matched_id?;

    // 先查 session_index 中的 thread_name
    let mut title = thread_names.get(&id).cloned().unwrap_or_default();

    // 如果 thread_name 为空，从后续行中找第一条真实用户消息
    if title.is_empty() {
        for line in lines_iter.take(30) {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let obj: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if obj.get("type").and_then(|t| t.as_str()) != Some("response_item") {
                continue;
            }
            if obj.pointer("/payload/role").and_then(|v| v.as_str()) != Some("user") {
                continue;
            }

            // 遍历 content blocks，找第一个非系统注入的 text
            if let Some(arr) = obj.pointer("/payload/content").and_then(|v| v.as_array()) {
                for item in arr {
                    if item.get("type").and_then(|t| t.as_str()) != Some("input_text") {
                        continue;
                    }
                    let text = item.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    let trimmed = text.trim_start();
                    if !trimmed.is_empty()
                        && !trimmed.starts_with('<')
                        && !trimmed.starts_with("# AGENTS.md")
                    {
                        title = trimmed.chars().take(100).collect();
                        break;
                    }
                }
            }
            if !title.is_empty() {
                break;
            }
        }

        if title.is_empty() {
            title = "Untitled".into();
        }
    }

    let timestamp = matched_timestamp;

    Some(AiSession {
        id,
        session_type: "codex".to_string(),
        title,
        timestamp,
    })
}

// ─── Tauri Command ─────────────────────────────────────────────

/// 同步扫描并返回排好序的 session 列表（供首次加载和后台刷新共用）
fn do_scan(project_path: &str) -> Vec<AiSession> {
    let mut sessions = Vec::new();
    sessions.extend(get_claude_sessions(project_path));
    sessions.extend(get_codex_sessions(project_path));
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    sessions
}

/// 后台静默刷新：扫完后写缓存并 emit sessions-updated 通知前端
fn refresh_in_background(app: AppHandle, cache: CacheMap, project_path: String) {
    thread::spawn(move || {
        let sessions = do_scan(&project_path);
        cache.lock().unwrap().insert(project_path.clone(), (sessions, Instant::now()));
        let _ = app.emit("sessions-updated", &project_path);
    });
}

/// 所有路径均立即返回（不阻塞主线程），通过 sessions-updated 事件推送最新数据：
/// - 缓存新鲜（<10min）：直接返回，不启动后台任务
/// - 缓存过期：立即返回旧数据 + 后台静默刷新
/// - 无缓存（首次）：立即返回空列表 + 后台扫描，完成后 emit sessions-updated
/// - force=true：立即返回当前缓存（可能为空）+ 强制后台刷新
#[tauri::command]
pub fn get_ai_sessions(
    app: AppHandle,
    cache: tauri::State<'_, SessionCache>,
    project_path: String,
    force: Option<bool>,
) -> Result<Vec<AiSession>, String> {
    let cache_arc = cache.inner.clone();
    let force = force.unwrap_or(false);

    let cached = {
        let guard = cache_arc.lock().unwrap();
        guard.get(&project_path).map(|(s, t)| (s.clone(), t.elapsed()))
    };

    match cached {
        Some((sessions, elapsed)) => {
            // 有缓存：立即返回；过期或强制时启动后台刷新
            if force || elapsed >= CACHE_TTL {
                refresh_in_background(app, cache_arc, project_path);
            }
            Ok(sessions)
        }
        None => {
            // 无缓存（首次加载）：立即返回空列表，后台扫描完成后通过 sessions-updated 通知
            refresh_in_background(app, cache_arc, project_path);
            Ok(vec![])
        }
    }
}
