use serde::Serialize;
#[cfg(any(windows, target_os = "macos"))]
use std::path::PathBuf;
#[cfg(any(windows, target_os = "macos"))]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(windows)]
const WINDOWS_DIR: &str = r"H:\workspace\self\claude\clipboard-images";
#[cfg(windows)]
const WSL_DIR: &str = "/workspace/h-workspace/self/claude/clipboard-images";
#[cfg(any(windows, target_os = "macos"))]
static IMAGE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPathImage {
    saved_path: String,
    terminal_path: String,
}

fn validate_rgba(rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "图片尺寸溢出".to_string())?;
    if rgba.len() != expected as usize {
        return Err(format!(
            "RGBA 长度不匹配: got {}, expected {}",
            rgba.len(),
            expected
        ));
    }
    Ok(())
}

#[cfg(any(windows, target_os = "macos"))]
fn image_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        Ok(PathBuf::from(WINDOWS_DIR))
    }
    #[cfg(target_os = "macos")]
    {
        dirs::cache_dir()
            .map(|dir| dir.join("mini-term").join("clipboard-images"))
            .ok_or_else(|| "无法获取 macOS 缓存目录".to_string())
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn next_file_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = IMAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("miniterm-{nanos}-{}-{sequence}.png", std::process::id())
}

#[cfg(any(windows, target_os = "macos"))]
fn save_png(rgba: &[u8], width: u32, height: u32) -> Result<PathBuf, String> {
    let dir = image_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建图片目录失败: {e}"))?;
    let path = dir.join(next_file_name());
    image::save_buffer(&path, rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| format!("保存 PNG 失败: {e}"))?;
    Ok(path)
}

#[cfg(any(windows, target_os = "macos"))]
fn build_result(path: PathBuf) -> Result<ClipboardPathImage, String> {
    #[cfg(windows)]
    let terminal_path = {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "无法生成 WSL 图片文件名".to_string())?;
        format!("{WSL_DIR}/{file_name}")
    };
    #[cfg(target_os = "macos")]
    let terminal_path = path.to_string_lossy().into_owned();

    Ok(ClipboardPathImage {
        saved_path: path.to_string_lossy().into_owned(),
        terminal_path,
    })
}

#[tauri::command]
pub fn save_clipboard_rgba_image_for_path_paste(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<ClipboardPathImage, String> {
    validate_rgba(&rgba, width, height)?;
    #[cfg(any(windows, target_os = "macos"))]
    {
        build_result(save_png(&rgba, width, height)?)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err("图片路径粘贴仅支持 Windows/macOS".into())
    }
}

#[tauri::command]
pub fn read_clipboard_image_for_path_paste() -> Result<ClipboardPathImage, String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        #[cfg(windows)]
        let temp_path = PathBuf::from(crate::clipboard::read_clipboard_image()?);
        #[cfg(target_os = "macos")]
        let temp_path = PathBuf::from(crate::clipboard::read_clipboard_image_macos()?);

        let image = image::open(&temp_path).map_err(|e| format!("读取临时剪贴板图片失败: {e}"))?;
        let rgba = image.to_rgba8();
        let result = build_result(save_png(rgba.as_raw(), image.width(), image.height())?);
        let _ = std::fs::remove_file(temp_path);
        result
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err("图片路径粘贴仅支持 Windows/macOS".into())
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn cleanup_old_images() {
    let Ok(dir) = image_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let threshold = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 60 * 60))
        .unwrap_or(std::time::UNIX_EPOCH);
    for entry in entries.flatten() {
        let path = entry.path();
        let is_owned_png = path.extension().and_then(|ext| ext.to_str()) == Some("png")
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("miniterm-"));
        if is_owned_png
            && entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified < threshold)
        {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn start_cleanup() {
    cleanup_old_images();
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(6 * 60 * 60));
        cleanup_old_images();
    });
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn start_cleanup() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_rgba_length() {
        assert!(validate_rgba(&[0, 0, 0], 1, 1).is_err());
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn generated_names_are_unique() {
        let first = next_file_name();
        let second = next_file_name();
        assert_ne!(first, second);
        assert!(first.starts_with("miniterm-"));
        assert!(first.ends_with(".png"));
    }

    #[cfg(windows)]
    #[test]
    fn maps_windows_file_name_to_wsl_directory() {
        let image = build_result(PathBuf::from(
            r"H:\workspace\self\claude\clipboard-images\miniterm-test.png",
        ))
        .unwrap();
        assert_eq!(
            image.terminal_path,
            "/workspace/h-workspace/self/claude/clipboard-images/miniterm-test.png"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_terminal_path_is_native_absolute_path() {
        let path = PathBuf::from("/Users/test/Library/Caches/mini-term/clipboard-images/test.png");
        let image = build_result(path.clone()).unwrap();
        assert_eq!(image.terminal_path, path.to_string_lossy().into_owned());
    }
}
