/// 图片剪贴板支持模块
///
/// 粘贴路径（按优先级）：
///   1. 前端 readImage() 像素 → save_clipboard_rgba_image（跨平台）
///   2. macOS NSPasteboard 原生兜底 → read_clipboard_image_macos
///   3. Windows CF_DIB/CF_BITMAP 原生兜底 → read_clipboard_image
///   4. 纯文本（前端处理）
///   5. Alt+V（最后保险）

use std::path::PathBuf;

// ── 公共 PNG 落盘 helper ───────────────────────────────────────────────────────

fn save_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("mini-term-clipboard");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("clip-{millis}.png"));

    image::save_buffer(&path, rgba, width, height, image::ColorType::Rgba8)
        .map_err(|e| format!("保存 PNG 失败: {e}"))?;

    Ok(path)
}

// ── Windows：CF_DIB / CF_BITMAP ───────────────────────────────────────────────

#[cfg(windows)]
mod win {
    use super::save_rgba_png;
    use std::path::PathBuf;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    const CF_DIB: u32 = 8;
    const CF_BITMAP: u32 = 2;

    pub fn read_clipboard_to_png() -> Result<PathBuf, String> {
        unsafe {
            OpenClipboard(None).map_err(|e| format!("打开剪贴板失败: {e}"))?;
            let result = try_read_clipboard();
            let _ = CloseClipboard();
            result
        }
    }

    unsafe fn try_read_clipboard() -> Result<PathBuf, String> {
        if IsClipboardFormatAvailable(CF_DIB).is_ok() {
            if let Ok(path) = read_cf_dib() {
                return Ok(path);
            }
        }
        if IsClipboardFormatAvailable(CF_BITMAP).is_ok() {
            if let Ok(path) = read_cf_bitmap() {
                return Ok(path);
            }
        }
        Err("剪贴板中没有可读取的图片格式（CF_DIB / CF_BITMAP）".into())
    }

    unsafe fn read_cf_dib() -> Result<PathBuf, String> {
        let hmem = GetClipboardData(CF_DIB)
            .map_err(|e| format!("GetClipboardData(CF_DIB) 失败: {e}"))?;

        let ptr = GlobalLock(HANDLE(hmem.0 as _));
        if ptr.is_null() {
            return Err("GlobalLock 失败".into());
        }
        let size = GlobalSize(HANDLE(hmem.0 as _));
        let data: Vec<u8> = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        let _ = GlobalUnlock(HANDLE(hmem.0 as _));

        if data.len() < std::mem::size_of::<BITMAPINFOHEADER>() {
            return Err("CF_DIB 数据过短".into());
        }

        let header = &*(data.as_ptr() as *const BITMAPINFOHEADER);
        let width = header.biWidth.unsigned_abs() as u32;
        let height = header.biHeight.unsigned_abs() as u32;
        let bit_count = header.biBitCount;

        if width == 0 || height == 0 {
            return Err("CF_DIB 图像尺寸为零".into());
        }

        let rgba = dib_to_rgba(&data, width, height, bit_count)?;
        save_rgba_png(&rgba, width, height)
    }

    unsafe fn read_cf_bitmap() -> Result<PathBuf, String> {
        let hmem = GetClipboardData(CF_BITMAP)
            .map_err(|e| format!("GetClipboardData(CF_BITMAP) 失败: {e}"))?;

        let hbitmap = HBITMAP(hmem.0 as _);
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return Err("CreateCompatibleDC 失败".into());
        }

        let old_obj = SelectObject(hdc, hbitmap.into());

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: 0,
                biHeight: 0,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            ..Default::default()
        };

        GetDIBits(hdc, hbitmap, 0, 0, None, &mut bi, DIB_RGB_COLORS);
        let width = bi.bmiHeader.biWidth.unsigned_abs();
        let height = bi.bmiHeader.biHeight.unsigned_abs();

        if width == 0 || height == 0 {
            SelectObject(hdc, old_obj);
            let _ = DeleteDC(hdc);
            return Err("CF_BITMAP 尺寸为零".into());
        }

        bi.bmiHeader.biWidth = width as i32;
        bi.bmiHeader.biHeight = -(height as i32);
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = 0;

        let row_bytes = (width * 4) as usize;
        let mut pixels = vec![0u8; row_bytes * height as usize];

        GetDIBits(
            hdc,
            hbitmap,
            0,
            height,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc, old_obj);
        let _ = DeleteDC(hdc);

        // Win32 32-bit DIB：BGRA → RGBA
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        save_rgba_png(&pixels, width, height)
    }

    fn dib_to_rgba(data: &[u8], width: u32, height: u32, bit_count: u16) -> Result<Vec<u8>, String> {
        let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
        let pixel_data = &data[header_size..];
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);

        match bit_count {
            24 => {
                let row_bytes = ((width * 3 + 3) & !3) as usize;
                for row in (0..height).rev() {
                    let row_start = row as usize * row_bytes;
                    for col in 0..width as usize {
                        let offset = row_start + col * 3;
                        if offset + 2 < pixel_data.len() {
                            rgba.push(pixel_data[offset + 2]); // R
                            rgba.push(pixel_data[offset + 1]); // G
                            rgba.push(pixel_data[offset]);     // B
                            rgba.push(255);                    // A
                        }
                    }
                }
            }
            32 => {
                let row_bytes = (width * 4) as usize;
                for row in (0..height).rev() {
                    let row_start = row as usize * row_bytes;
                    for col in 0..width as usize {
                        let offset = row_start + col * 4;
                        if offset + 3 < pixel_data.len() {
                            rgba.push(pixel_data[offset + 2]); // R
                            rgba.push(pixel_data[offset + 1]); // G
                            rgba.push(pixel_data[offset]);     // B
                            rgba.push(pixel_data[offset + 3]); // A
                        }
                    }
                }
            }
            _ => return Err(format!("不支持的位深度: {bit_count}，仅支持 24/32")),
        }

        Ok(rgba)
    }
}

// ── macOS：NSPasteboard 原生兜底 ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod mac {
    use super::save_rgba_png;
    use std::ffi::c_void;
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;

    pub fn read_clipboard_to_png() -> Result<PathBuf, String> {
        unsafe {
            let pb_cls = AnyClass::get(c"NSPasteboard")
                .ok_or_else(|| "NSPasteboard class not found".to_string())?;

            let pb: Retained<AnyObject> = msg_send![pb_cls, generalPasteboard];

            // TIFF 是 macOS 剪贴板的首选格式；PNG 作为备选
            for uti in &["public.tiff", "public.png"] {
                let str_cls = AnyClass::get(c"NSString")
                    .ok_or_else(|| "NSString class not found".to_string())?;

                let c_uti = std::ffi::CString::new(*uti).unwrap();
                let ns_uti: Retained<AnyObject> =
                    msg_send![str_cls, stringWithUTF8String: c_uti.as_ptr()];

                let data_opt: Option<Retained<AnyObject>> =
                    msg_send![&pb, dataForType: &*ns_uti];

                let Some(data) = data_opt else { continue };

                let length: usize = msg_send![&data, length];
                if length == 0 {
                    continue;
                }

                let bytes: *const c_void = msg_send![&data, bytes];
                if bytes.is_null() {
                    continue;
                }

                let raw = std::slice::from_raw_parts(bytes as *const u8, length);
                match image::load_from_memory(raw) {
                    Ok(img) => {
                        let rgba = img.to_rgba8();
                        return save_rgba_png(rgba.as_raw(), img.width(), img.height());
                    }
                    Err(_) => continue,
                }
            }
        }

        Err("NSPasteboard 中没有可用的图片数据（TIFF / PNG）".into())
    }
}

// ── 启动清理 ──────────────────────────────────────────────────────────────────

/// 清理 mini-term-clipboard 目录中超过 24 小时的旧 PNG 文件。
/// 启动时调用一次；读目录/删文件失败均静默跳过。
pub fn cleanup_old_clipboard_images() {
    let dir = std::env::temp_dir().join("mini-term-clipboard");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };

    let threshold = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(24 * 60 * 60))
        .unwrap_or(std::time::UNIX_EPOCH);

    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified < threshold {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

// ── Tauri commands ────────────────────────────────────────────────────────────

/// 跨平台：把前端传来的 RGBA 像素落盘成临时 PNG，返回文件路径。
#[tauri::command]
pub fn save_clipboard_rgba_image(rgba: Vec<u8>, width: u32, height: u32) -> Result<String, String> {
    let expected = width
        .checked_mul(height)
        .and_then(|px| px.checked_mul(4))
        .ok_or_else(|| "图片尺寸溢出".to_string())?;

    if rgba.len() != expected as usize {
        return Err(format!(
            "RGBA 长度不匹配: got {}, expected {}",
            rgba.len(),
            expected
        ));
    }

    let path = save_rgba_png(&rgba, width, height)?;
    Ok(path.to_string_lossy().into_owned())
}

/// macOS 原生兜底：通过 NSPasteboard 读取 TIFF/PNG，保存为临时 PNG，返回路径。
#[tauri::command]
pub fn read_clipboard_image_macos() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let path = mac::read_clipboard_to_png()?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("仅支持 macOS 平台".into())
    }
}

/// Windows 原生兜底：通过 CF_DIB/CF_BITMAP 读取，保存为临时 PNG，返回路径。
#[tauri::command]
pub fn read_clipboard_image() -> Result<String, String> {
    #[cfg(windows)]
    {
        let path = win::read_clipboard_to_png()?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(windows))]
    {
        Err("图片剪贴板 Win32 兜底仅支持 Windows 平台".into())
    }
}
