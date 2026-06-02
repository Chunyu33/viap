// Windows 可执行文件图标提取模块
// 使用 Win32 API 从 .exe/.dll 中提取图标，编码为 Base64 PNG 供前端渲染
//
// 缓存架构（两级）：
//   1. 内存缓存 ICON_CACHE — 进程内命中，避免重复 ExtractIconExW
//   2. 磁盘缓存 %APPDATA%/viap/cache/icons/{sha1}.png — 跨进程/重启命中
//      自动失效：sha1(exe_path + icon_index + modified_time)，exe 升级后 mtime 变化 → 新 key

use std::path::{Path, PathBuf};
use std::io::Cursor;
use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use sha1::{Sha1, Digest};

#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
#[cfg(windows)]
use windows::Win32::UI::Shell::ExtractIconExW;
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    GetDIBits, CreateCompatibleDC, DeleteDC, SelectObject, GetObjectW,
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject,
};
#[cfg(windows)]
use windows::core::PCWSTR;

// 图标内存缓存：键为图标路径（如 "C:\app.exe,0"），值为 Base64 编码的 PNG
lazy_static::lazy_static! {
    static ref ICON_CACHE: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());
}

/// 解析图标路径，分离文件路径和图标索引
///
/// # 示例
/// - "C:\app.exe" -> ("C:\app.exe", 0)
/// - "C:\app.exe,0" -> ("C:\app.exe", 0)
/// - "C:\app.exe,-101" -> ("C:\app.exe", -101)
#[cfg(windows)]
fn parse_icon_path(icon_path: &str) -> (String, i32) {
    let path = icon_path.trim().trim_matches('"');

    if let Some(comma_pos) = path.rfind(',') {
        let file_part = &path[..comma_pos];
        let index_part = &path[comma_pos + 1..];
        if let Ok(index) = index_part.trim().parse::<i32>() {
            return (file_part.trim().trim_matches('"').to_string(), index);
        }
    }

    (path.trim_matches('"').to_string(), 0)
}

// ============================================================================
// 磁盘缓存辅助函数
// ============================================================================

/// 获取图标磁盘缓存目录，不存在时自动创建
#[cfg(windows)]
fn get_icon_cache_dir() -> Option<PathBuf> {
    let dir = crate::storage::data_dir::get_data_dir().join("cache").join("icons");
    if !dir.exists() {
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
    }
    Some(dir)
}

/// 计算图标磁盘缓存 key
/// sha1(exe_lowercase_path + ":" + icon_index + ":" + unix_modified_seconds) → 40 字符 hex
/// icon_index 必须参与计算：同一 DLL 的不同索引对应不同图标（如 shell32.dll,0 vs shell32.dll,5）
/// exe 升级后 mtime 变化 → key 自动变化 → 旧缓存自然失效
#[cfg(windows)]
fn compute_icon_cache_key(exe_path: &str, icon_index: i32) -> Option<String> {
    let path = Path::new(exe_path);
    if !path.exists() {
        return None;
    }
    let modified = path.metadata().ok()?.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    // 路径统一小写：Windows 大小写不敏感，避免同一文件产生不同 key
    let input = format!("{}:{}:{}", exe_path.to_lowercase(), icon_index, secs);
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

/// 尝试从磁盘缓存读取图标 PNG 字节
/// 返回 Some(Vec<u8>) 表示命中，None 表示未命中或文件损坏
#[cfg(windows)]
fn read_disk_cache(exe_path: &str, icon_index: i32) -> Option<Vec<u8>> {
    let key = compute_icon_cache_key(exe_path, icon_index)?;
    let file_path = get_icon_cache_dir()?.join(format!("{}.png", key));
    if !file_path.exists() {
        return None;
    }
    std::fs::read(&file_path).ok()
}

/// 将图标 PNG 字节写入磁盘缓存
/// 原子写入：先写临时文件（含进程 PID 防多进程冲突），再原子 rename 到目标
/// 已存在则跳过：target.exists() 前置检查 + rename 失败容忍并发
#[cfg(windows)]
fn save_disk_cache(exe_path: &str, icon_index: i32, png_bytes: &[u8]) {
    let key = match compute_icon_cache_key(exe_path, icon_index) {
        Some(k) => k,
        None => return,
    };
    let cache_dir = match get_icon_cache_dir() {
        Some(d) => d,
        None => return,
    };
    let target = cache_dir.join(format!("{}.png", key));
    // 已存在则不重复写入（另一个线程可能先写入了）
    if target.exists() {
        return;
    }
    // tmp 文件名含 PID，防止多个 Viap 进程同时提取同一图标时互相覆盖
    let tmp = cache_dir.join(format!("{}.{}.png.tmp", key, std::process::id()));
    if std::fs::write(&tmp, png_bytes).is_ok() {
        // rename 失败忽略（另一个线程/进程可能同时写入并先 rename 了）
        let _ = std::fs::rename(&tmp, &target);
        // 清理残留 tmp（rename 失败时 tmp 仍存在）
        let _ = std::fs::remove_file(&tmp);
    }
}

// ============================================================================
// 图标提取核心
// ============================================================================

/// 将 HICON 图标句柄转换为 (Base64 PNG, 原始PNG字节)
///
/// # 技术实现
/// 1. GetIconInfo — 获取图标的颜色位图和掩码位图
/// 2. GetDIBits — 将位图转换为 BGRA 像素数据
/// 3. BGRA → RGBA 转换
/// 4. image crate 编码为 PNG
/// 5. 返回 (base64_data_uri, raw_png_bytes)
#[cfg(windows)]
fn icon_to_base64(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> (String, Vec<u8>) {
    unsafe {
        let mut icon_info = ICONINFO::default();
        if GetIconInfo(icon, &mut icon_info).is_err() {
            return (String::new(), Vec::new());
        }

        let hbm_color = icon_info.hbmColor;
        if hbm_color.is_invalid() {
            if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }
            return (String::new(), Vec::new());
        }

        // 获取位图尺寸
        let mut bitmap = BITMAP::default();
        let bitmap_size = std::mem::size_of::<BITMAP>() as i32;
        if GetObjectW(hbm_color, bitmap_size, Some(&mut bitmap as *mut _ as *mut _)) == 0 {
            let _ = DeleteObject(hbm_color);
            if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }
            return (String::new(), Vec::new());
        }

        let width = bitmap.bmWidth as u32;
        let height = bitmap.bmHeight as u32;

        // 限制图标大小，防止处理异常大图标
        if width == 0 || height == 0 || width > 256 || height > 256 {
            let _ = DeleteObject(hbm_color);
            if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }
            return (String::new(), Vec::new());
        }

        // 创建设备上下文并选择位图
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            let _ = DeleteObject(hbm_color);
            if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }
            return (String::new(), Vec::new());
        }

        let old_bitmap = SelectObject(hdc, hbm_color);

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // 负值 = 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default(); 1],
        };

        let pixel_count = (width * height) as usize;
        let mut pixels: Vec<u8> = vec![0; pixel_count * 4];

        let result = GetDIBits(
            hdc, hbm_color, 0, height,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi, DIB_RGB_COLORS,
        );

        // 清理 GDI 资源
        SelectObject(hdc, old_bitmap);
        let _ = DeleteDC(hdc);
        let _ = DeleteObject(hbm_color);
        if !icon_info.hbmMask.is_invalid() { let _ = DeleteObject(icon_info.hbmMask); }

        if result == 0 { return (String::new(), Vec::new()); }

        // BGRA → RGBA
        for i in 0..pixel_count {
            let offset = i * 4;
            pixels.swap(offset, offset + 2);
        }

        // 编码为 PNG，同时返回 Base64 字符串和原始 PNG 字节
        match image::RgbaImage::from_raw(width, height, pixels) {
            Some(img) => {
                let mut png_data = Cursor::new(Vec::new());
                if img.write_to(&mut png_data, image::ImageFormat::Png).is_ok() {
                    let png_bytes = png_data.into_inner();
                    let base64_str = BASE64_STANDARD.encode(&png_bytes);
                    (format!("data:image/png;base64,{}", base64_str), png_bytes)
                } else {
                    (String::new(), Vec::new())
                }
            }
            None => (String::new(), Vec::new()),
        }
    }
}

/// 从 EXE/DLL 文件中提取图标并转换为 Base64 编码的 PNG
///
/// # 缓存架构
/// 1. 内存缓存 ICON_CACHE — 进程内命中，最快
/// 2. 磁盘缓存 %APPDATA%/viap/cache/icons/{sha1}.png — 跨重启命中
///    key = sha1(exe_path + icon_index + exe_modified_time)，exe 升级后自动失效
/// 3. ExtractIconExW — 均未命中时执行提取，结果同时写入两级缓存
///
/// # 参数
/// - `icon_path`: 图标路径，可能包含索引（如 "C:\app.exe,0"）
///
/// # 返回
/// - 成功时返回 `data:image/png;base64,...` 格式字符串
/// - 失败时返回空字符串
#[cfg(windows)]
pub fn extract_icon_to_base64(icon_path: &str) -> String {
    // 一级缓存：内存（ICON_CACHE）
    if let Ok(cache) = ICON_CACHE.lock() {
        if let Some(cached) = cache.get(icon_path) {
            println!("[icon] mem hit  {}", icon_path);
            return cached.clone();
        }
    }

    let (file_path, icon_index) = parse_icon_path(icon_path);

    if !Path::new(&file_path).exists() {
        return String::new();
    }

    // 二级缓存：磁盘（%APPDATA%/viap/cache/icons/{sha1}.png）
    if let Some(png_bytes) = read_disk_cache(&file_path, icon_index) {
        println!("[icon] disk hit {}", icon_path);
        let base64_str = BASE64_STANDARD.encode(&png_bytes);
        let result = format!("data:image/png;base64,{}", base64_str);
        // 回填内存缓存
        if let Ok(mut cache) = ICON_CACHE.lock() {
            cache.insert(icon_path.to_string(), result.clone());
        }
        return result;
    }

    let wide_path: Vec<u16> = file_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let mut large_icon = windows::Win32::UI::WindowsAndMessaging::HICON::default();

        let result = ExtractIconExW(
            PCWSTR::from_raw(wide_path.as_ptr()),
            icon_index,
            Some(&mut large_icon),
            None,
            1,
        );

        if result == 0 || large_icon.is_invalid() {
            return String::new();
        }

        let (base64_result, png_bytes) = icon_to_base64(large_icon);
        let _ = DestroyIcon(large_icon);

        // 写入两级缓存
        if !base64_result.is_empty() {
            println!("[icon] extract {}", icon_path);
            // 磁盘缓存：保存原始 PNG 字节（跨重启命中）
            save_disk_cache(&file_path, icon_index, &png_bytes);
            // 内存缓存：保存 Base64 字符串（进程内命中）
            if let Ok(mut cache) = ICON_CACHE.lock() {
                cache.insert(icon_path.to_string(), base64_result.clone());
            }
        }

        base64_result
    }
}

/// 从 EXE/DLL 文件中提取图标的原始 PNG 字节（供自定义协议使用）
///
/// 与 extract_icon_to_base64 不同，此函数返回原始 PNG 字节，
/// 避免 Base64 编解码开销，直接用于 Tauri custom protocol 响应。
///
/// # 返回
/// - 成功时返回 PNG 字节数据
/// - 失败时返回空 Vec
/// 从 exe 提取图标 PNG 原始字节（供自定义协议使用，预留）
#[cfg(windows)]
#[allow(dead_code)]
pub fn extract_icon_png_bytes(icon_path: &str) -> Vec<u8> {
    let base64_str = extract_icon_to_base64(icon_path);
    if base64_str.is_empty() {
        return Vec::new();
    }
    // 去掉 "data:image/png;base64," 前缀
    let b64_body = base64_str.strip_prefix("data:image/png;base64,").unwrap_or(&base64_str);
    BASE64_STANDARD.decode(b64_body).unwrap_or_default()
}

#[cfg(not(windows))]
pub fn extract_icon_png_bytes(_icon_path: &str) -> Vec<u8> {
    Vec::new()
}

/// 非 Windows 平台的图标提取占位函数
#[cfg(not(windows))]
pub fn extract_icon_to_base64(_icon_path: &str) -> String {
    String::new()
}
