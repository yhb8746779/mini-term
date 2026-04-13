/// 最小性能日志模块 — 追加写入 {app_data_dir}/perf.log
/// 格式：`2026-04-13 11:02:31.125 | scope=get_git_status | project=H:\xxx | cost_ms=842 | ...`
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

fn perf_log_path(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().expect("app data dir");
    std::fs::create_dir_all(&dir).ok();
    dir.join("perf.log")
}

/// 将 Unix epoch (secs + millis) 格式化为 UTC 日期时间字符串
/// 使用 Howard Hinnant 公历算法，不依赖外部 crate
fn epoch_to_datetime(secs: u64, millis: u32) -> String {
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;

    let z = days as i64 + 719468;
    let era = if z >= 0 { z / 146097 } else { (z - 146096) / 146097 };
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if mo <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        yr, mo, d, h, m, s, millis
    )
}

fn format_ts() -> String {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    epoch_to_datetime(dur.as_secs(), dur.subsec_millis())
}

/// 追加一行原始文本（自动加换行）
pub fn append_perf(app: &AppHandle, line: String) {
    let path = perf_log_path(app);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", line);
    }
}

/// 构建标准格式日志行并追加
/// `<ts> | scope=<scope> | <details>`
pub fn log_perf(app: &AppHandle, scope: &str, details: &str) {
    let line = format!("{} | scope={} | {}", format_ts(), scope, details);
    append_perf(app, line);
}

// ─── Tauri commands ────────────────────────────────────────────

/// 清空 perf.log（测试前调用）
#[tauri::command]
pub fn clear_perf_log(app: AppHandle) -> Result<(), String> {
    let path = perf_log_path(&app);
    std::fs::write(&path, "").map_err(|e| e.to_string())
}

/// 读取 perf.log 全文（用于开发调试查看）
#[tauri::command]
pub fn read_perf_log(app: AppHandle) -> Result<String, String> {
    let path = perf_log_path(&app);
    if !path.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// 前端打点入口（switch_start / switch_paint_done 等客户端事件）
#[tauri::command]
pub fn log_perf_from_frontend(app: AppHandle, scope: String, details: String) -> Result<(), String> {
    log_perf(&app, &scope, &details);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_to_datetime_known_date() {
        // 2026-04-13 00:00:00 UTC
        // Days from 1970-01-01 to 2026-04-13 = 20556
        let secs: u64 = 20556 * 86400;
        let result = epoch_to_datetime(secs, 0);
        assert_eq!(result, "2026-04-13 00:00:00.000");
    }

    #[test]
    fn epoch_to_datetime_with_millis() {
        // 2026-04-13 11:02:31.125 UTC
        let secs: u64 = 20556 * 86400 + 11 * 3600 + 2 * 60 + 31;
        let result = epoch_to_datetime(secs, 125);
        assert_eq!(result, "2026-04-13 11:02:31.125");
    }

    #[test]
    fn epoch_to_datetime_unix_epoch() {
        // 1970-01-01 00:00:00.000
        let result = epoch_to_datetime(0, 0);
        assert_eq!(result, "1970-01-01 00:00:00.000");
    }
}
