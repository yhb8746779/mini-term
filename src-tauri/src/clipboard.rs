/// 图片剪贴板支持模块
///
/// - Windows：通过 Win32 API 读取 CF_DIB / CF_BITMAP 并保存为临时 PNG
/// - 非 Windows：直接返回错误；前端捕获后退回 Alt+V（让 AI 工具自己读图）

#[cfg(windows)]
mod win {
    use std::path::PathBuf;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HBITMAP,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::Foundation::HANDLE;

    // 标准剪贴板格式常量
    const CF_DIB: u32 = 8;
    const CF_BITMAP: u32 = 2;

    pub fn read_clipboard_to_png() -> Result<PathBuf, String> {
        unsafe {
            // 打开剪贴板（NULL hwnd 表示当前线程）
            OpenClipboard(None).map_err(|e| format!("打开剪贴板失败: {e}"))?;

            let result = try_read_clipboard();

            // 无论成功失败都关闭剪贴板
            let _ = CloseClipboard();
            result
        }
    }

    unsafe fn try_read_clipboard() -> Result<std::path::PathBuf, String> {
        // 优先尝试 CF_DIB（设备无关位图，包含完整颜色信息）
        if IsClipboardFormatAvailable(CF_DIB).is_ok() {
            if let Ok(path) = read_cf_dib() {
                return Ok(path);
            }
        }

        // 再尝试 CF_BITMAP（与设备相关的 HBITMAP）
        if IsClipboardFormatAvailable(CF_BITMAP).is_ok() {
            if let Ok(path) = read_cf_bitmap() {
                return Ok(path);
            }
        }

        Err("剪贴板中没有可读取的图片格式（CF_DIB / CF_BITMAP）".into())
    }

    unsafe fn read_cf_dib() -> Result<std::path::PathBuf, String> {
        let hmem = GetClipboardData(CF_DIB)
            .map_err(|e| format!("GetClipboardData(CF_DIB) 失败: {e}"))?;

        let ptr = GlobalLock(HANDLE(hmem.0 as _));
        if ptr.is_null() {
            return Err("GlobalLock 失败".into());
        }
        let size = GlobalSize(HANDLE(hmem.0 as _));

        // 将内存复制到 Vec，之后立即释放锁
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
        save_png(rgba, width, height)
    }

    unsafe fn read_cf_bitmap() -> Result<std::path::PathBuf, String> {
        let hmem = GetClipboardData(CF_BITMAP)
            .map_err(|e| format!("GetClipboardData(CF_BITMAP) 失败: {e}"))?;

        let hbitmap = HBITMAP(hmem.0 as _);

        // 先查询尺寸：用空 BITMAPINFOHEADER 调用 GetDIBits
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
                biCompression: 0, // BI_RGB
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            ..Default::default()
        };

        // 第一次调用获取尺寸
        GetDIBits(hdc, hbitmap, 0, 0, None, &mut bi, DIB_RGB_COLORS);
        let width = bi.bmiHeader.biWidth.unsigned_abs();
        let height = bi.bmiHeader.biHeight.unsigned_abs();

        if width == 0 || height == 0 {
            SelectObject(hdc, old_obj);
            let _ = DeleteDC(hdc);
            return Err("CF_BITMAP 尺寸为零".into());
        }

        bi.bmiHeader.biWidth = width as i32;
        bi.bmiHeader.biHeight = -(height as i32); // 负值 = top-down
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = 0; // BI_RGB

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

        // Win32 32-bit DIB 是 BGRA（B G R A），转 RGBA
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2); // B <-> R
        }

        save_png(pixels, width, height)
    }

    fn dib_to_rgba(data: &[u8], width: u32, height: u32, bit_count: u16) -> Result<Vec<u8>, String> {
        let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
        let pixel_data = &data[header_size..];
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);

        match bit_count {
            24 => {
                // 3 bytes per pixel, bottom-up, rows padded to 4 bytes
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
                // 4 bytes per pixel, bottom-up
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

    fn save_png(rgba: Vec<u8>, width: u32, height: u32) -> Result<std::path::PathBuf, String> {
        let dir = std::env::temp_dir().join("mini-term-clipboard");
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败: {e}"))?;

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let path = dir.join(format!("clip-{millis}.png"));

        image::save_buffer(&path, &rgba, width, height, image::ColorType::Rgba8)
            .map_err(|e| format!("保存 PNG 失败: {e}"))?;

        Ok(path)
    }
}

/// 清理 mini-term-clipboard 目录中超过 24 小时的旧 PNG 文件。
/// 启动时调用一次；读目录/删文件失败均静默跳过，不阻断应用启动。
pub fn cleanup_old_clipboard_images() {
    let dir = std::env::temp_dir().join("mini-term-clipboard");
    let Ok(entries) = std::fs::read_dir(&dir) else { return };

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

/// 从剪贴板读取图片并保存为临时 PNG，返回文件路径。
/// 仅 Windows 平台实现；非 Windows 返回错误，前端会退回 Alt+V。
#[tauri::command]
pub fn read_clipboard_image() -> Result<String, String> {
    #[cfg(windows)]
    {
        let path = win::read_clipboard_to_png()?;
        Ok(path.to_string_lossy().into_owned())
    }
    #[cfg(not(windows))]
    {
        Err("图片剪贴板仅支持 Windows 平台".into())
    }
}
