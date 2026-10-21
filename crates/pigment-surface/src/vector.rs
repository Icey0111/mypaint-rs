//! SVG 矢量层的存储与栅格化 —— `lib/layer/data.py:1153-1239` 的 `VectorLayer`。
//!
//! 源码的 VectorLayer 是 `FileBackedLayer`：权威数据是一个 `.svg` 工作文件，
//! 表面只保存它的位图预览（`data.py:597-609`）。本实现保留同样的分工：
//! [`VectorLayerData`] 持有 SVG 文本与文档坐标，`Layer` 的 `Painting` 表面
//! 保存 resvg 栅格化出的预览。
//!
//! 与源码的差异：
//! - 源码的临时工作文件/修订目录（`_ManagedFile`、`revisions/`）由外部编辑
//!   状态在 app 层管理；这里只存 SVG 文本。
//! - GdkPixbuf 的 SVG loader 用 librsvg；这里用 resvg，尺寸都取 SVG 的
//!   `width`/`height`（源码 `load_surface_from_pixbuf_file(..., image_type="svg")`）。

use std::sync::Arc;

use crate::PigmentSurface;

/// 单个矢量层的权威数据。
#[derive(Clone, Debug, PartialEq)]
pub struct VectorLayerData {
    /// SVG 源文本。
    pub svg: Arc<str>,
    /// SVG 左上角在模型坐标里的位置（`FileBackedLayer._x/_y`）。
    pub x: i32,
    pub y: i32,
}

impl VectorLayerData {
    pub fn new(svg: impl Into<Arc<str>>, x: i32, y: i32) -> Self {
        Self {
            svg: svg.into(),
            x,
            y,
        }
    }
}

/// SVG 栅格化的最大边长（防止损坏/恶意文件一次分配巨量内存）。
pub const MAX_SVG_SIDE: u32 = 16384;
/// 最大像素数（16384² 太大，限制到 64M 像素 ≈ 256 MiB RGBA）。
pub const MAX_SVG_PIXELS: u64 = 64 * 1024 * 1024;

/// 把 SVG 栅格化成表面，放在模型坐标 `(x, y)`。
///
/// 对应 `FileBackedLayer._ensure_valid_working_file()` 里的
/// `load_surface_from_pixbuf_file(rev0_fp.name, x=self._x, y=self._y)`
/// （`data.py:628-648`）。空/损坏的 SVG 返回错误。
pub fn rasterize_svg(svg: &str, x: i32, y: i32) -> Result<PigmentSurface, String> {
    let tree = resvg::usvg::Tree::from_str(svg, &resvg::usvg::Options::default())
        .map_err(|e| format!("SVG 解析失败：{e}"))?;
    let size = tree.size();
    let w = size.width().ceil().max(1.0) as u32;
    let h = size.height().ceil().max(1.0) as u32;
    if w > MAX_SVG_SIDE || h > MAX_SVG_SIDE || (w as u64) * (h as u64) > MAX_SVG_PIXELS {
        return Err(format!("SVG 尺寸过大：{w}×{h}"));
    }
    let mut pixmap =
        resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| format!("SVG 位图分配失败：{w}×{h}"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    // tiny-skia 的像素是预乘 RGBA8；表面需要非预乘的 8 位输入，
    // 由 `replace_from_rgba8` 再做 sRGB→fix15 转换。
    let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.data().chunks_exact(4) {
        let a = px[3] as u32;
        if a == 0 {
            rgba.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let demul = |c: u8| -> u8 { (((c as u32) * 255 + a / 2) / a).min(255) as u8 };
            rgba.extend_from_slice(&[demul(px[0]), demul(px[1]), demul(px[2]), a as u8]);
        }
    }
    let mut surface = PigmentSurface::new();
    surface.replace_from_rgba8(&rgba, x, y, w, h);
    Ok(surface)
}

/// 生成新矢量层的初始 SVG —— 照 `data.py:1194-1222` 的
/// `write_blank_backing_file()`：当前视口四角围成的随机色虚线轮廓。
///
/// `points` 是**相对** SVG 原点 `(0, 0)` 的坐标；`w/h` 是 SVG 尺寸（= 视口
/// 四角的外接矩形，`document.py:1427-1433`）。
pub fn blank_vector_svg(w: i32, h: i32, points: &[(f32, f32)], color: [u8; 3]) -> String {
    let w = w.max(1);
    let h = h.max(1);
    let mut svg = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\
         <!-- Created by MyPaint (http://mypaint.org/) -->\
         <svg version=\"1.1\" width=\"{w}\" height=\"{h}\"><path d=\"M "
    );
    for (i, (x, y)) in points.iter().enumerate() {
        // SVG 允许 moveto 后省略 `L`（源码就是这么写的），但 resvg 的
        // path 解析器对省略写法只画最后一段；显式 `L` 语义完全相同。
        if i == 0 {
            svg.push_str(&format!("{x:.6},{y:.6} "));
        } else {
            svg.push_str(&format!("L {x:.6},{y:.6} "));
        }
    }
    svg.push_str(&format!(
        "Z\" id=\"path0\" style=\"fill:none;stroke:#{:02x}{:02x}{:02x};stroke-width:5;\
         stroke-linecap:round;stroke-linejoin:round;stroke-dasharray:9, 9;\
         stroke-dashoffset:0\" /></svg>",
        color[0], color[1], color[2]
    ));
    svg
}

/// 源码 `randint(0x33, 0x99)` 的等价随机色（不引入 rand 依赖）。
pub fn random_outline_color() -> [u8; 3] {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mut seed = nanos
        .wrapping_mul(6364136223846793005)
        .wrapping_add(n.wrapping_mul(1442695040888963407))
        .wrapping_add(std::process::id() as u64);
    let mut out = [0u8; 3];
    for c in &mut out {
        // 线性同余，取 0..=0x66 再偏移 0x33。
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *c = 0x33 + ((seed >> 33) % 0x67) as u8;
    }
    out
}

/// 视口四角的模型坐标 → `rotated_rectangle_bbox()` 的整数外接框。
///
/// 逐行照 `lib/helpers.py:234-241`：`floor(min)` / `floor(max)`，宽高加 1。
pub fn rotated_rectangle_bbox(corners: &[(f32, f32); 4]) -> (i32, i32, i32, i32) {
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for (x, y) in corners {
        min_x = min_x.min(*x);
        min_y = min_y.min(*y);
        max_x = max_x.max(*x);
        max_y = max_y.max(*y);
    }
    let x1 = min_x.floor() as i32;
    let y1 = min_y.floor() as i32;
    let x2 = max_x.floor() as i32;
    let y2 = max_y.floor() as i32;
    (x1, y1, x2 - x1 + 1, y2 - y1 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_a_red_rectangle_at_the_requested_origin() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="6">
            <rect x="0" y="0" width="8" height="6" fill="#ff0000"/></svg>"##;
        let s = rasterize_svg(svg, 100, 50).unwrap();
        // 表面外接框是 tile 对齐的（`Layer::data_bbox` 的绘画层分支）。
        assert_eq!(s.data_bbox(), (64, 0, 64, 64));
        let px = s.tile(100 / 64, 50 / 64).unwrap();
        let ix = (100 % 64) as usize;
        let iy = (50 % 64) as usize;
        assert!(px[iy][ix][3] > 30000, "alpha 应接近满值");
        assert!(px[iy][ix][0] > px[iy][ix][1]);
    }

    #[test]
    fn rejects_malformed_and_oversized_svg() {
        assert!(rasterize_svg("<svg>", 0, 0).is_err());
        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20000" height="4"/>"#;
        assert!(rasterize_svg(huge, 0, 0).is_err());
    }

    #[test]
    fn blank_svg_matches_source_shape() {
        let svg = blank_vector_svg(
            120,
            80,
            &[(0.0, 0.0), (120.0, 0.0), (120.0, 80.0), (0.0, 80.0)],
            [0x44, 0x55, 0x66],
        );
        assert!(svg.starts_with("<?xml version=\"1.0\""));
        assert!(svg.contains("width=\"120\" height=\"80\""));
        assert!(svg.contains("M 0.000000,0.000000 L 120.000000,0.000000"));
        assert!(svg.contains("stroke:#445566"));
        assert!(svg.contains("stroke-dasharray:9, 9"));
        // 生成物本身必须能被自己的栅格化器读回。
        let s = rasterize_svg(&svg, 0, 0).unwrap();
        assert_eq!(s.data_bbox(), (0, 0, 128, 128));
    }

    #[test]
    fn explicit_stroked_path_draws_all_four_edges() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="80">
            <path d="M 5,5 L 95,5 L 95,75 L 5,75 Z" fill="none" stroke="#4080c0" stroke-width="5"/></svg>"##;
        let s = rasterize_svg(&svg, 0, 0).unwrap();
        let alpha_at = |x: i32, y: i32| {
            s.tile(x.div_euclid(64), y.div_euclid(64)).unwrap()[(y.rem_euclid(64)) as usize]
                [(x.rem_euclid(64)) as usize][3]
        };
        for (x, y) in [(6, 40), (94, 40), (50, 6), (50, 74)] {
            assert!(alpha_at(x, y) > 1000, "边 ({x},{y}) 没画出来");
        }
    }

    #[test]
    fn generated_outline_uses_explicit_lineto_and_inset_points() {
        let svg = blank_vector_svg(
            100,
            80,
            &[(5.0, 5.0), (95.0, 5.0), (95.0, 75.0), (5.0, 75.0)],
            [0x40, 0x80, 0xc0],
        );
        assert!(svg.contains("M 5.000000,5.000000 L 95.000000,5.000000"));
        let s = rasterize_svg(&svg, 0, 0).unwrap();
        let alpha_at = |x: i32, y: i32| {
            s.tile(x.div_euclid(64), y.div_euclid(64)).unwrap()[(y.rem_euclid(64)) as usize]
                [(x.rem_euclid(64)) as usize][3]
        };
        // 虚线路径在某一 y 上可能正好落在 gap；沿边取最大值。
        let edge_max = |fixed: i32, vertical: bool| {
            (5..76)
                .map(|v| {
                    if vertical {
                        alpha_at(fixed, v)
                    } else {
                        alpha_at(v, fixed)
                    }
                })
                .max()
                .unwrap_or(0)
        };
        assert!(edge_max(5, true) > 1000, "左边没画出来");
        assert!(edge_max(95, true) > 1000, "右边没画出来");
        assert!(edge_max(5, false) > 1000, "上边没画出来");
        assert!(edge_max(75, false) > 1000, "下边没画出来");
    }

    #[test]
    fn rotated_bbox_uses_floor_and_inclusive_extent() {
        // 未旋转、非整数：floor(0.2)=0, floor(9.8)=9 → 宽 10
        let bbox = rotated_rectangle_bbox(&[(0.2, 0.2), (9.8, 0.2), (9.8, 4.9), (0.2, 4.9)]);
        assert_eq!(bbox, (0, 0, 10, 5));
        // 旋转 45° 的菱形：外接框是正方形
        let bbox = rotated_rectangle_bbox(&[(0.0, -5.0), (5.0, 0.0), (0.0, 5.0), (-5.0, 0.0)]);
        assert_eq!(bbox, (-5, -5, 11, 11));
    }

    #[test]
    fn random_color_stays_in_source_range() {
        for _ in 0..64 {
            let c = random_outline_color();
            assert!(c.iter().all(|v| (0x33..=0x99).contains(v)), "{c:?}");
        }
    }
}
