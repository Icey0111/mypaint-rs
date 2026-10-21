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
pub fn write_rgba8_png(path: &Path, pixels: Vec<u8>, w: u32, h: u32) -> Result<(), String> {
    write_png(path, pixels, w, h, true)
}

/// 按源码 `save_as_png(alpha=...)` 写 PNG，并带 `sRGB`/`gAMA`/`cHRM`
/// 三个辅助块（`ProgressivePNGWriter(..., save_srgb_chunks=True)`）。
///
/// `alpha=false` 时写 **RGB8**（丢弃 alpha 通道，与 libpng 的 RGB 写出
/// 一致），`alpha=true` 时写 RGBA8。
pub fn write_png(path: &Path, pixels: Vec<u8>, w: u32, h: u32, alpha: bool) -> Result<(), String> {
    let expected = (w as usize) * (h as usize) * 4;
    if pixels.len() != expected {
        return Err(format!(
            "像素缓冲尺寸不匹配：{w}×{h}，期望 {expected} 字节，实际 {}",
            pixels.len()
        ));
    }
    let data: Vec<u8> = if alpha {
        pixels
    } else {
        // RGB8：丢掉每像素第 4 字节
        let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for p in pixels.chunks_exact(4) {
            rgb.extend_from_slice(&p[..3]);
        }
        rgb
    };
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_color(if alpha {
            png::ColorType::Rgba
        } else {
            png::ColorType::Rgb
        });
        enc.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        enc.set_source_gamma(png::ScaledFloat::from_scaled(45455));
        enc.set_source_chromaticities(png::SourceChromaticities::new(
            (0.3127, 0.3290),
            (0.6400, 0.3300),
            (0.3000, 0.6000),
            (0.1500, 0.0600),
        ));
        let mut writer = enc
            .write_header()
            .map_err(|e| format!("写 PNG 头失败 {}：{e}", path.display()))?;
        writer
            .write_image_data(&data)
            .map_err(|e| format!("写 PNG 数据失败 {}：{e}", path.display()))?;
        writer
            .finish()
            .map_err(|e| format!("收尾 PNG 失败 {}：{e}", path.display()))?;
    }
    std::fs::write(path, out).map_err(|e| format!("写 PNG 失败 {}：{e}", path.display()))
}

/// 读取 PNG 为 RGBA8 像素，返回 `(pixels, w, h)`。
///
/// `image` crate 默认给解码器设了约 512 MiB 的内存上限。无限画布的一张
/// 18k×10k 图层 PNG 本身就是 745 MiB，会被误判成恶意图片而拒绝载入
/// （报 `Memory limit exceeded`）。文档载入路径必须显式关掉这个上限。
pub fn read_rgba8_png(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("读 PNG 失败 {}：{e}", path.display()))?;
    reader.limits(image::Limits::no_limits());
    let img = reader
        .decode()
        .map_err(|e| format!("读 PNG 失败 {}：{e}", path.display()))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok((rgba.into_raw(), w, h))
}

/// 从内存字节解码 PNG（测试与 zip 内嵌场景）。
///
/// 与 [`read_rgba8_png`] 一样关掉 `image` 的默认分配上限：ORA 的图层
/// PNG 可以合法地超过 512 MiB。
pub fn decode_rgba8_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32), String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
    reader.set_format(image::ImageFormat::Png);
    reader.limits(image::Limits::no_limits());
    let img = reader.decode().map_err(|e| format!("解码 PNG 失败：{e}"))?;
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

    /// 源码 `save_srgb_chunks=True`：sRGB + 回退 gAMA/cHRM 都应写出；
    /// `alpha=false` 时 IHDR 的 color type 必须是 2（RGB），不是 6。
    #[test]
    fn writes_srgb_chunks_and_rgb_for_opaque() {
        fn chunks(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
            let mut out = Vec::new();
            let mut i = 8;
            while i + 8 <= bytes.len() {
                let len = u32::from_be_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
                let ty = String::from_utf8_lossy(&bytes[i + 4..i + 8]).into_owned();
                let data = bytes[i + 8..i + 8 + len].to_vec();
                out.push((ty, data));
                i += 12 + len;
            }
            out
        }
        let path = tmp("srgb.png");
        let px = vec![255, 0, 0, 255, 0, 255, 0, 255];
        write_png(&path, px, 2, 1, false).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let cs = chunks(&bytes);
        let names: Vec<&str> = cs.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"sRGB"), "缺 sRGB 块: {names:?}");
        assert!(names.contains(&"gAMA"), "缺 gAMA 块: {names:?}");
        assert!(names.contains(&"cHRM"), "缺 cHRM 块: {names:?}");
        let ihdr = &cs.iter().find(|(n, _)| n == "IHDR").unwrap().1;
        assert_eq!(ihdr[9], 2, "alpha=false 应写 RGB (color type 2)");
        // 解码回来仍是 RGB，且像素值不变
        let img = image::open(&path).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (2, 1));
        assert_eq!(&img.into_raw()[..3], &[255, 0, 0]);
        let _ = std::fs::remove_file(&path);

        // alpha=true 仍是 RGBA
        let path = tmp("srgb-rgba.png");
        write_png(&path, vec![1, 2, 3, 4], 1, 1, true).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let cs = chunks(&bytes);
        let ihdr = &cs.iter().find(|(n, _)| n == "IHDR").unwrap().1;
        assert_eq!(ihdr[9], 6, "alpha=true 应写 RGBA (color type 6)");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_reports_error() {
        let err = read_rgba8_png(Path::new("definitely-not-here-xyz.png"));
        assert!(err.is_err());
    }
}
