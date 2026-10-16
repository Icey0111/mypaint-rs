//! 诊断工具：把 CPU 侧 tile 表面导出为 PNG。
//!
//! 用途：区分「CPU 里本来就没画上」与「画上了但显存没同步」——按 `F2` 导出，
//! 再与屏幕画面逐处对照即可定位。M6 的 OpenRaster 导出也会复用这里的拼合逻辑。

use std::path::PathBuf;

use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_surface::PigmentSurface;

use crate::convert;

/// 把整个稀疏画布（已分配 tile 的外接矩形）拼合导出为 PNG。
/// 返回写入的路径。
pub fn dump_surface_png(surface: &PigmentSurface) -> Result<PathBuf, String> {
    let coords: Vec<(i32, i32)> = surface.tiles().map(|(c, _)| c).collect();
    if coords.is_empty() {
        return Err("画布为空，没有可导出的 tile".into());
    }
    let ts = TILE_SIZE as i32;
    let min_tx = coords.iter().map(|c| c.0).min().unwrap();
    let min_ty = coords.iter().map(|c| c.1).min().unwrap();
    let max_tx = coords.iter().map(|c| c.0).max().unwrap();
    let max_ty = coords.iter().map(|c| c.1).max().unwrap();

    let w = ((max_tx - min_tx + 1) * ts) as u32;
    let h = ((max_ty - min_ty + 1) * ts) as u32;
    // 底噪用画布纸色而不是纯白：本诊断的用途是与**屏幕画面**对照，
    // 而屏幕上空 tile 显示的是 `CANVAS_BG`（M2 时还是纯白，这里已同步）。
    let mut img = image::RgbaImage::from_pixel(
        w,
        h,
        image::Rgba([convert::CANVAS_BG[0], convert::CANVAS_BG[1], convert::CANVAS_BG[2], 255]),
    );

    for (coord, tile) in surface.tiles() {
        let px = convert::tile_to_rgba8(tile.pixels(), convert::CANVAS_BG);
        let ox = (coord.0 - min_tx) * ts;
        let oy = (coord.1 - min_ty) * ts;
        for y in 0..ts {
            for x in 0..ts {
                let i = ((y * ts + x) as usize) * 4;
                img.put_pixel(
                    (ox + x) as u32,
                    (oy + y) as u32,
                    image::Rgba([px[i], px[i + 1], px[i + 2], 255]),
                );
            }
        }
    }

    let path = PathBuf::from("pigment_canvas_dump.png");
    img.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
