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
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, GetDIBits, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
        OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
    };

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

        let hglobal = HGLOBAL(hmem.0 as *mut _);
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            return Err("GlobalLock 失败".into());
        }
        let size = GlobalSize(hglobal);
        let data: Vec<u8> = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        let _ = GlobalUnlock(hglobal);

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

        let old_obj = SelectObject(hdc, HGDIOBJ(hbitmap.0 as _));

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

    /// 从剪贴板读取 CF_HDROP 文件路径列表。
    /// CF_HDROP 是 Windows 资源管理器复制文件时写入剪贴板的格式。
    pub fn read_cf_hdrop_paths() -> Result<Vec<String>, String> {
        unsafe {
            OpenClipboard(None).map_err(|e| format!("打开剪贴板失败: {e}"))?;
            let result = do_read_cf_hdrop();
            let _ = CloseClipboard();
            result
        }
    }

    unsafe fn do_read_cf_hdrop() -> Result<Vec<String>, String> {
        const CF_HDROP: u32 = 15;

        if IsClipboardFormatAvailable(CF_HDROP).is_err() {
            return Err("剪贴板中没有 CF_HDROP 格式".into());
        }

        let hmem = GetClipboardData(CF_HDROP)
            .map_err(|e| format!("GetClipboardData(CF_HDROP) 失败: {e}"))?;

        let hglobal = HGLOBAL(hmem.0 as *mut _);
        let ptr = GlobalLock(hglobal);
        if ptr.is_null() {
            return Err("GlobalLock 失败".into());
        }

        let size = GlobalSize(hglobal);
        let data: &[u8] = std::slice::from_raw_parts(ptr as *const u8, size);
        let result = parse_dropfiles(data);
        let _ = GlobalUnlock(hglobal);
        result
    }

    /// 从磁盘图片文件加载图像，写入 Windows 剪贴板为 CF_DIB 格式，
    /// 供 AI CLI 的 Alt+V 图片粘贴快捷键读取（与截图位图路径完全一致）。
    pub fn write_image_file_to_clipboard(path: &str) -> Result<(), String> {
        // 读取图片文件并解码为 RGB8
        let img = image::open(path).map_err(|e| format!("读取图片文件失败: {e}"))?;
        let rgb = img.to_rgb8();
        let width = rgb.width();
        let height = rgb.height();
        let pixels = rgb.as_raw(); // RGB8，top-down

        // CF_DIB 布局：BITMAPINFOHEADER + 24-bit BGR 像素数据（bottom-up，行 4 字节对齐）
        let row_stride = ((width * 3 + 3) & !3) as usize;
        let pixel_bytes = row_stride * height as usize;
        let header_size = std::mem::size_of::<BITMAPINFOHEADER>();
        let total_size = header_size + pixel_bytes;

        let mut dib = vec![0u8; total_size];

        // 写 BITMAPINFOHEADER（biHeight 正数 = bottom-up）
        let header = BITMAPINFOHEADER {
            biSize: header_size as u32,
            biWidth: width as i32,
            biHeight: height as i32,
            biPlanes: 1,
            biBitCount: 24,
            biCompression: 0, // BI_RGB
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                &header as *const BITMAPINFOHEADER as *const u8,
                dib.as_mut_ptr(),
                header_size,
            );
        }

        // RGB8 top-down → BGR bottom-up
        let pixel_buf = &mut dib[header_size..];
        for row in 0..height as usize {
            let src_row = height as usize - 1 - row; // 垂直翻转
            let dst = row * row_stride;
            let src = src_row * width as usize * 3;
            for col in 0..width as usize {
                let s = src + col * 3;
                let d = dst + col * 3;
                pixel_buf[d]     = pixels[s + 2]; // B
                pixel_buf[d + 1] = pixels[s + 1]; // G
                pixel_buf[d + 2] = pixels[s];     // R
            }
            // 行末填充字节已经是 0（vec 初始化）
        }

        // 写入 Windows 剪贴板
        unsafe {
            const CF_DIB: u32 = 8;

            OpenClipboard(None).map_err(|e| format!("打开剪贴板失败: {e}"))?;
            let result = (|| -> Result<(), String> {
                EmptyClipboard().map_err(|e| format!("EmptyClipboard 失败: {e}"))?;

                let hmem = GlobalAlloc(GMEM_MOVEABLE, total_size)
                    .map_err(|e| format!("GlobalAlloc 失败: {e}"))?;
                let ptr = GlobalLock(hmem);
                if ptr.is_null() {
                    return Err("GlobalLock 失败".into());
                }
                std::ptr::copy_nonoverlapping(dib.as_ptr(), ptr as *mut u8, total_size);
                let _ = GlobalUnlock(hmem);

                // 将 HGLOBAL 所有权移交给剪贴板（之后不可再 GlobalFree）
                SetClipboardData(CF_DIB, HANDLE(hmem.0 as *mut _))
                    .map_err(|e| format!("SetClipboardData 失败: {e}"))?;
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }
    }

    fn parse_dropfiles(data: &[u8]) -> Result<Vec<String>, String> {
        use std::ffi::OsString;
        use std::os::windows::ffi::OsStringExt;

        // DROPFILES 结构布局（Windows SDK）：
        //   0..4:   pFiles (DWORD) — 文件列表相对 DROPFILES 起始的字节偏移
        //   4..12:  pt (POINT)     — 拖放坐标（忽略）
        //   12..16: fNC (BOOL)     — 非客户区标志（忽略）
        //   16..20: fWide (BOOL)   — 1 = UTF-16 文件路径；0 = ANSI
        if data.len() < 20 {
            return Err("CF_HDROP 数据过短".into());
        }

        let p_files = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let f_wide = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) != 0;

        if p_files >= data.len() {
            return Err("CF_HDROP pFiles 偏移越界".into());
        }

        let file_data = &data[p_files..];
        let mut paths: Vec<String> = Vec::new();

        if f_wide {
            // UTF-16LE 空终止字符串列表，双空结束
            let words: Vec<u16> = file_data
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            let mut start = 0;
            loop {
                if start >= words.len() || words[start] == 0 {
                    break;
                }
                let end = words[start..]
                    .iter()
                    .position(|&c| c == 0)
                    .map(|p| start + p)
                    .unwrap_or(words.len());
                let os = OsString::from_wide(&words[start..end]);
                paths.push(os.to_string_lossy().into_owned());
                start = end + 1;
            }
        } else {
            // ANSI 空终止字符串列表
            let mut start = 0;
            loop {
                if start >= file_data.len() || file_data[start] == 0 {
                    break;
                }
                let end = file_data[start..]
                    .iter()
                    .position(|&b| b == 0)
                    .map(|p| start + p)
                    .unwrap_or(file_data.len());
                let s = String::from_utf8_lossy(&file_data[start..end]).into_owned();
                paths.push(s);
                start = end + 1;
            }
        }

        if paths.is_empty() {
            Err("CF_HDROP 中没有文件路径".into())
        } else {
            Ok(paths)
        }
    }
}

// ── macOS：NSPasteboard 原生兜底 ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod mac {
    use super::save_rgba_png;
    use std::ffi::{c_char, c_void, CString};
    use std::path::PathBuf;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;

    // ── Finder 文件路径读取 ────────────────────────────────────────────────

    /// 从 NSPasteboard 读取 public.file-url 格式的 Finder 文件路径列表。
    /// Finder 复制文件时会把文件 URL（file:///path/to/file）写入此格式。
    ///
    /// 注意：某些文件的 public.file-url 是 `file:///.file:id=XXXX` 格式（Finder 内部节点 ID）。
    /// 这种格式无法直接作为文件路径使用，通过 NSURL.path 解析后过滤掉 `/.file:` 开头的结果。
    pub fn read_finder_file_paths() -> Result<Vec<String>, String> {
        unsafe {
            let pb_cls = AnyClass::get(c"NSPasteboard")
                .ok_or_else(|| "NSPasteboard class not found".to_string())?;

            let pb: Retained<AnyObject> = msg_send![pb_cls, generalPasteboard];

            let items_opt: Option<Retained<AnyObject>> = msg_send![&pb, pasteboardItems];
            let items = items_opt.ok_or_else(|| "pasteboardItems returned nil".to_string())?;

            let count: usize = msg_send![&items, count];
            if count == 0 {
                return Err("No pasteboard items".into());
            }

            let str_cls = AnyClass::get(c"NSString")
                .ok_or_else(|| "NSString class not found".to_string())?;
            let url_cls = AnyClass::get(c"NSURL")
                .ok_or_else(|| "NSURL class not found".to_string())?;

            let c_type = CString::new("public.file-url").unwrap();
            let ns_type: Retained<AnyObject> =
                msg_send![str_cls, stringWithUTF8String: c_type.as_ptr()];

            let mut paths: Vec<String> = Vec::new();

            for i in 0..count {
                let item: Retained<AnyObject> = msg_send![&items, objectAtIndex: i];
                let url_str_opt: Option<Retained<AnyObject>> =
                    msg_send![&item, stringForType: &*ns_type];

                let Some(url_str) = url_str_opt else { continue };

                // 用 NSURL 解析 URL，再取 .path，让系统处理 percent-encoding
                // 注意：file:///.file:id=XXXX 是 Finder 节点 ID，.path 返回 /.file:id=XXXX，
                //       这种路径无法直接使用，在下方过滤掉。
                let url_opt: Option<Retained<AnyObject>> =
                    msg_send![url_cls, URLWithString: &*url_str];
                let Some(url) = url_opt else { continue };

                let path_opt: Option<Retained<AnyObject>> = msg_send![&url, path];
                let Some(path_obj) = path_opt else { continue };

                let utf8: *const c_char = msg_send![&path_obj, UTF8String];
                if utf8.is_null() {
                    continue;
                }

                let path = std::ffi::CStr::from_ptr(utf8)
                    .to_string_lossy()
                    .into_owned();

                // 过滤 Finder 节点 ID（/.file:id=XXXX），不是可用的文件系统路径
                if path.starts_with("/.file:") {
                    continue;
                }

                if !path.is_empty() {
                    paths.push(path);
                }
            }

            if paths.is_empty() {
                Err("No usable file paths found in pasteboard".into())
            } else {
                Ok(paths)
            }
        }
    }

    /// 从磁盘图片文件加载图像，写入 macOS NSPasteboard 为 TIFF 格式，
    /// 供 AI CLI 的 Ctrl+V / Alt+V 图片粘贴读取（与截图位图路径一致）。
    pub fn write_image_file_to_clipboard(path: &str) -> Result<(), String> {
        // 读取并解码图片文件
        let img = image::open(path).map_err(|e| format!("读取图片文件失败: {e}"))?;

        // 编码为 TIFF（NSPasteboard 首选格式，Claude/Codex 读取时优先尝试）
        let mut tiff_cursor = std::io::Cursor::new(Vec::<u8>::new());
        img.write_to(&mut tiff_cursor, image::ImageFormat::Tiff)
            .map_err(|e| format!("TIFF 编码失败: {e}"))?;
        let tiff_data = tiff_cursor.into_inner();

        unsafe {
            let pb_cls = AnyClass::get(c"NSPasteboard")
                .ok_or_else(|| "NSPasteboard class not found".to_string())?;
            let pb: Retained<AnyObject> = msg_send![pb_cls, generalPasteboard];

            // 清空剪贴板（clearContents 返回变化计数）
            let _: i64 = msg_send![&pb, clearContents];

            let str_cls = AnyClass::get(c"NSString")
                .ok_or_else(|| "NSString class not found".to_string())?;
            let data_cls = AnyClass::get(c"NSData")
                .ok_or_else(|| "NSData class not found".to_string())?;

            // NSPasteboardTypeTIFF = "public.tiff"
            let c_tiff = CString::new("public.tiff").unwrap();
            let ns_tiff_type: Retained<AnyObject> =
                msg_send![str_cls, stringWithUTF8String: c_tiff.as_ptr()];

            // NSData from bytes
            let ns_data: Retained<AnyObject> = msg_send![data_cls,
                dataWithBytes: tiff_data.as_ptr() as *const c_void,
                length: tiff_data.len()];

            // setData:forType: returns BOOL
            let ok: i8 = msg_send![&pb, setData: &*ns_data, forType: &*ns_tiff_type];
            if ok == 0 {
                return Err("NSPasteboard setData:forType: 写入失败".into());
            }

            Ok(())
        }
    }

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

/// 读取 Windows 剪贴板中的 CF_HDROP 文件路径列表。
/// 资源管理器复制文件/图片文件时使用此格式，返回文件路径字符串数组。
/// 非 Windows 平台或剪贴板没有 CF_HDROP 时返回 Err。
#[tauri::command]
pub fn read_clipboard_file_paths() -> Result<Vec<String>, String> {
    #[cfg(windows)]
    {
        win::read_cf_hdrop_paths()
    }
    #[cfg(not(windows))]
    {
        Err("CF_HDROP 仅支持 Windows 平台".into())
    }
}

/// 读取 macOS 剪贴板中的 Finder 文件路径列表（public.file-url）。
/// Finder 复制文件/图片文件时使用此格式，返回本地路径字符串数组。
/// 非 macOS 平台或剪贴板没有 file URL 时返回 Err。
#[tauri::command]
pub fn read_clipboard_file_paths_macos() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        mac::read_finder_file_paths()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("Finder 文件路径读取仅支持 macOS 平台".into())
    }
}

/// 从磁盘图片文件加载图像并写入系统剪贴板。
///
/// Windows → CF_DIB（24-bit BGR bottom-up）
/// macOS   → NSPasteboard TIFF（public.tiff）
///
/// 写入成功后，前端可像截图粘贴一样触发 Alt+V（Windows/Codex）或 Ctrl+V（macOS+Claude），
/// AI CLI 将从剪贴板读取图片数据，以图片块方式展示（而非路径文本）。
///
/// 与截图位图路径的区别：截图是剪贴板中已有位图；此命令是先从文件加载再写入剪贴板。
#[tauri::command]
pub fn load_image_to_clipboard(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::write_image_file_to_clipboard(&path)
    }
    #[cfg(target_os = "macos")]
    {
        mac::write_image_file_to_clipboard(&path)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Err(format!("load_image_to_clipboard 仅支持 Windows/macOS，path={path}"))
    }
}
