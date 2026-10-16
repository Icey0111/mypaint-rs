//! PNG 编解码 —— 服务 `save_as_png` / `load_png`。
//!
//! MyPaint 自己用 C++ 的 `ProgressivePNGWriter`（渐进写出，省内存）
//! 与 `GdkPixbuf` 读入。Rust 侧直接用 `image` crate 的 PNG 能力，
//! **语义对齐**：
//!
//! - 写：`alpha=false` → RGB8（无 alpha 通道）；`alpha=true` → RGBA8。
//!   这与源码 `mypaintlib.ProgressivePNGWriter(fp, w, h, alpha, ...)`
//!   的 `alpha` 参数含义一致（`lib/surface.py:275`）。
//! - 读：不预设位深/色型，统一转成 RGBA8。MyPaint 侧走 GdkPixbuf，
//!   同样接受 RGB/RGBA/灰度/调色板等（`lib/pixbuf.py:load_from_file`）。
//!
//! 未移植：sRGB/cHRM/gAMA 辅助块（源码的 `save_srgb_chunks` 开关，
//! 默认写）。属元数据，不影响像素；`.ora` 里的 PNG 是否需要它另议。

use std::path::Path;

/// 写出 RGBA8 像素为 PNG。
///
/// `pixels` 必须是 `w * h * 4` 字节、行主序。
pub fn write_rgba8_png(
    path: &Path,
    pixels: Vec<u8>,
    w: u32,
    h: u32,
) -> Result<(), String> {
    debug_assert_eq!(pixels.len(), (w as usize) * (h as usize) * 4);
    let img: image::RgbaImage = image::ImageBuffer::from_raw(w, h, pixels)
        .ok_or_else(|| format!("像素缓冲尺寸不匹配：{w}×{h}"))?;
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| format!("写 PNG 失败 {}：{e}", path.display()))
}

/// 读取 PNG 为 RGBA8 像素，返回 `(pixels, w, h)`。
pub fn read_rgba8_png(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::open(path).map_err(|e| format!("读 PNG 失败 {}：{e}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// 从内存字节解码 PNG（测试与 zip 内嵌场景）。
pub fn decode_rgba8_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|e| format!("解码 PNG 失败：{e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("pigment-png-test-{}-{}", std::process::id(), name));
        p
    }

    #[test]
    fn rgba_round_trips_through_file() {
        let path = tmp("rgba.png");
        let (w, h) = (3u32, 2u32);
        let px: Vec<u8> = (0..(w * h * 4)).map(|i| (i * 7 % 256) as u8).collect();
        write_rgba8_png(&path, px.clone(), w, h).unwrap();
        let (back, bw, bh) = read_rgba8_png(&path).unwrap();
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, px, "RGBA 往返必须逐字节一致");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decodes_from_memory() {
        let (w, h) = (2u32, 2u32);
        let px = vec![255u8; (w * h * 4) as usize];
        let path = tmp("mem.png");
        write_rgba8_png(&path, px.clone(), w, h).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let (back, bw, bh) = decode_rgba8_png(&bytes).unwrap();
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, px);
        let _ = std::fs::remove_file(&path);
    }

    /// PNG 魔数：确认我们写的确实是 PNG。
    #[test]
    fn writes_a_real_png() {
        let path = tmp("magic.png");
        write_rgba8_png(&path, vec![0u8; 4], 1, 1).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reports_error() {
        let err = read_rgba8_png(Path::new("definitely-not-here-xyz.png"));
        assert!(err.is_err());
    }
}
