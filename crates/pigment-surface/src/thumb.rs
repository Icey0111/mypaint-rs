//! 缩略图的共用数学 —— `lib/layer/tree.py:564 render_layer_preview` 的
//! mipmap 等价物。
//!
//! MyPaint 的 mipmap 是**逐级** box 平均：每次把 2×2 个值合成 1 个，
//! 每通道 `v0/4 + v1/4 + v2/4 + v3/4` —— 注意是**各自整除后再相加**，
//! 不是先求和再除（`lib/pixops.cpp:30 tile_downscale_rgba16_c`：
//! `dst_p[0] = src_p[0]/4 + (src_p+4)[0]/4 + ...`）。差 1 个 LSB 就不同，
//! 所以这里照抄这个顺序。
//!
//! 全程在 fix15 **预乘**整数域（tile 的原生格式）做，只在最后转 8 位时
//! 去预乘 + EOTF —— 与 MyPaint「mipmap 存在 15 位域、渲染时才转 8 位」
//! 的结构一致。把平均放到 8 位 sRGB 域做会整体偏亮。

use crate::color;

/// mipmap 层数上限（`libmypaint/mypaint-config.h:13 MYPAINT_MAX_MIPMAP_LEVEL`）。
pub const MAX_MIPMAP_LEVEL: u32 = 4;

/// 把 `2^level × 2^level` 的源像素块按 mipmap 规则平均成 1 个 fix15 像素。
///
/// `src(dx, dy)` 取块内相对坐标 `(0..2^level)` 的源像素；表外/空 tile 一律
/// 返回全零（`transparent_tile`）。
fn average_block<F>(level: u32, mut src: F) -> [u16; 4]
where
    F: FnMut(usize, usize) -> [u16; 4],
{
    let f = 1usize << level;
    // level ≤ 4 → 最多 16×16 像素，栈上放得下，但为免大栈帧用堆
    let mut buf = vec![0u16; f * f * 4];
    for dy in 0..f {
        for dx in 0..f {
            let p = src(dx, dy);
            let o = (dy * f + dx) * 4;
            buf[o..o + 4].copy_from_slice(&p);
        }
    }

    // 逐级减半（原地：目标行始终落在尚未读的源行之前）
    let mut cur = f;
    while cur > 1 {
        let half = cur / 2;
        for y in 0..half {
            for x in 0..half {
                let o00 = ((y * 2) * cur + x * 2) * 4;
                let o01 = o00 + 4;
                let o10 = o00 + cur * 4;
                let o11 = o10 + 4;
                let mut out = [0u16; 4];
                for c in 0..4 {
                    out[c] =
                        buf[o00 + c] / 4 + buf[o01 + c] / 4 + buf[o10 + c] / 4 + buf[o11 + c] / 4;
                }
                let od = (y * half + x) * 4;
                buf[od..od + 4].copy_from_slice(&out);
            }
        }
        cur = half;
    }
    [buf[0], buf[1], buf[2], buf[3]]
}

/// 把 `rect`（**level 0** 的模型坐标 `(x, y, w, h)`）按 `level` 级减半渲染成
/// RGBA8，返回 `(pixels, w, h)`，其中 `w = max(1, w >> level)`。
///
/// 坐标按 mipmap 语义**先减半再取块**（`x >> level << level`）：源码也是先
/// `x //= 2` 再把减半后的矩形交给 mipmap 层，所以块边界对齐的是 mipmap 网格，
/// 不是 bbox。`>>` 对负数是算术移位（= Python 的 `//`，向下取整）。
///
/// `encode` 负责最后一步 fix15 → 8 位（直通 alpha 或合成底色）。
pub fn render_halved<F, E>(
    rect: (i32, i32, i32, i32),
    level: u32,
    mut encode: E,
    mut pixel_src: F,
) -> (Vec<u8>, u32, u32)
where
    F: FnMut(i32, i32) -> [u16; 4],
    E: FnMut([u16; 4]) -> [u8; 4],
{
    let (x, y, w, h) = rect;
    let xh = x >> level;
    let yh = y >> level;
    let tw = (w >> level).max(1) as usize;
    let th = (h >> level).max(1) as usize;
    let mut out = vec![0u8; tw * th * 4];
    for oy in 0..th {
        for ox in 0..tw {
            let bx = (xh + ox as i32) << level;
            let by = (yh + oy as i32) << level;
            let avg = average_block(level, |dx, dy| pixel_src(bx + dx as i32, by + dy as i32));
            let px = encode(avg);
            let o = (oy * tw + ox) * 4;
            out[o..o + 4].copy_from_slice(&px);
        }
    }
    (out, tw as u32, th as u32)
}

/// 预览格里的棋盘格（`gui/layerprops.py:405-429` 的 `composite_color_simple`）：
/// `check_size` 从 2 起翻倍，直到 `>= preview_size/6` 或 `>= 16`。
pub fn checker_size(preview_size: i32) -> i32 {
    let mut check_size = 2;
    while check_size < preview_size / 6 && check_size < 16 {
        check_size *= 2;
    }
    check_size
}

/// 预览格棋盘的两种颜色 —— **实测** `0xFF707070` / `0xFF808080` 是
/// `0xAARRGGBB` 编码（Alpha=0xFF，RGB=0x70/0x80），即不透明
/// `(112,112,112)` / `(128,128,128)`。
pub const CHECKER_DARK: [u8; 3] = [0x70, 0x70, 0x70];
pub const CHECKER_LIGHT: [u8; 3] = [0x80, 0x80, 0x80];

/// 棋盘格在 `(px, py)` 处的颜色（`(x/check + y/check) % 2`，0 → 深色）。
///
/// 实测（GTK 2.x gdk-pixbuf）：`(0,0)` 深、`(8,0)` 亮、`(0,8)` 亮、`(8,8)` 深。
pub fn checker_color(px: i32, py: i32, check_size: i32) -> [u8; 3] {
    let cx = px.div_euclid(check_size).rem_euclid(2);
    let cy = py.div_euclid(check_size).rem_euclid(2);
    if (cx + cy) % 2 == 0 {
        CHECKER_DARK
    } else {
        CHECKER_LIGHT
    }
}

/// 缩略图（≤256）在 `preview_size` 方格里按 `make_preview` 的几何缩放并居中：
/// `scale = preview_size / max(w, h)`，返回目标尺寸与左上角位置（整数，向下取整）。
pub fn preview_geometry(w: i32, h: i32, preview_size: i32) -> (i32, i32, i32, i32) {
    let scale = preview_size as f32 / w.max(h).max(1) as f32;
    // 源码里 w、h 变成浮点后交给 GI 转 int（向零截断）
    let dw = (w as f32 * scale) as i32;
    let dh = (h as f32 * scale) as i32;
    let x = (preview_size - dw) / 2;
    let y = (preview_size - dh) / 2;
    (x, y, dw, dh)
}

/// 把 `src`（`sw × sh` RGBA8）合成到棋盘格上，得到 `preview_size` 方格
/// （`gui/layerprops.py:400 make_preview` 的像素级等价物）。
///
/// 缩放用**双线性**（源码 `InterpType.BILINEAR`）：`thumb.composite(...)` 把
/// 缩略图缩放到 `dw × dh` 再贴到 `(x, y)`。
pub fn compose_preview(
    src: &[u8],
    sw: i32,
    sh: i32,
    preview_size: i32,
) -> Vec<u8> {
    let ps = preview_size.max(1);
    let check = checker_size(ps);
    let mut out = vec![0u8; (ps * ps * 4) as usize];
    // 1) 棋盘底（不透明）
    for py in 0..ps {
        for px in 0..ps {
            let c = checker_color(px, py, check);
            let o = ((py * ps + px) * 4) as usize;
            out[o] = c[0];
            out[o + 1] = c[1];
            out[o + 2] = c[2];
            out[o + 3] = 255;
        }
    }
    // 2) 缩略图双线性缩放到 dw×dh，居中贴上去（底不透明 → over 后 alpha 仍 255）
    let (dx, dy, dw, dh) = preview_geometry(sw, sh, ps);
    if dw <= 0 || dh <= 0 || sw <= 0 || sh <= 0 {
        return out;
    }
    for ty in 0..dh {
        for tx in 0..dw {
            // 目标像素中心 → 源坐标（半像素中心对齐，同 pixops）
            let fx = (tx as f32 + 0.5) * (sw as f32 / dw as f32) - 0.5;
            let fy = (ty as f32 + 0.5) * (sh as f32 / dh as f32) - 0.5;
            let p = bilinear(src, sw, sh, fx, fy);
            let (px, py) = (dx + tx, dy + ty);
            if px < 0 || py < 0 || px >= ps || py >= ps {
                continue;
            }
            let a = p[3] as u32;
            if a == 0 {
                continue;
            }
            let o = ((py * ps + px) * 4) as usize;
            // 非预乘 source-over（目的不透明）
            for c in 0..3 {
                let s = p[c] as u32 * a + out[o + c] as u32 * (255 - a);
                out[o + c] = (s / 255).min(255) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

/// 非预乘 RGBA8 的双线性采样（边缘钳位）。
fn bilinear(src: &[u8], sw: i32, sh: i32, fx: f32, fy: f32) -> [u8; 4] {
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let at = |x: i32, y: i32| -> [u8; 4] {
        let xc = x.clamp(0, sw - 1);
        let yc = y.clamp(0, sh - 1);
        let o = ((yc * sw + xc) * 4) as usize;
        [src[o], src[o + 1], src[o + 2], src[o + 3]]
    };
    let p00 = at(x0, y0);
    let p10 = at(x0 + 1, y0);
    let p01 = at(x0, y0 + 1);
    let p11 = at(x0 + 1, y0 + 1);
    let mut out = [0u8; 4];
    for c in 0..4 {
        let top = p00[c] as f32 * (1.0 - tx) + p10[c] as f32 * tx;
        let bot = p01[c] as f32 * (1.0 - tx) + p11[c] as f32 * tx;
        out[c] = (top * (1.0 - ty) + bot * ty).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// fix15 直通 alpha（去预乘 + EOTF）—— 图层缩略图用。
#[inline]
pub fn encode_alpha(p: [u16; 4]) -> [u8; 4] {
    color::tile_to_rgba8_alpha_single(p)
}

/// fix15 合成到底色（不透明）—— 栈缩略图（`.ora` 预览）用。
#[inline]
pub fn encode_opaque(p: [u16; 4], bg: [u8; 3]) -> [u8; 4] {
    color::tile_to_rgba8_single(p, bg, color::bg_to_linear(bg))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 平均是「各自整除后再相加」：4 个 3 → 0（不是 12/4 = 3）。
    #[test]
    fn halving_divides_each_tap_before_adding() {
        let avg = average_block(1, |_, _| [3, 3, 3, 3]);
        assert_eq!(avg, [0, 0, 0, 0]);

        let avg = average_block(1, |_, _| [4, 8, 12, 16]);
        assert_eq!(avg, [4, 8, 12, 16], "每个 4 的倍数恰好整除");
    }

    /// 逐级减半：level 2 等价于先 level 1 再 level 1。
    #[test]
    fn halving_is_applied_level_by_level() {
        let v = |i: usize| [i as u16 * 100, 0, 0, 0];
        let avg2 = average_block(2, |dx, dy| v(dy * 4 + dx));
        // 先对 16 个值做一级（各 /4 相加），再对 4 个中间值做一级
        let mut lvl1 = [0u16; 4];
        for dy in 0..2 {
            for dx in 0..2 {
                let a = v(dy * 2 * 4 + dx * 2);
                let b = v(dy * 2 * 4 + dx * 2 + 1);
                let c = v((dy * 2 + 1) * 4 + dx * 2);
                let d = v((dy * 2 + 1) * 4 + dx * 2 + 1);
                let o = dy * 2 + dx;
                lvl1[o] = a[0] / 4 + b[0] / 4 + c[0] / 4 + d[0] / 4;
            }
        }
        let mut expected = 0u16;
        for x in lvl1 {
            expected += x / 4;
        }
        assert_eq!(avg2[0], expected);
    }

    /// 实测对齐（真 GTK 2.x gdk-pixbuf）：`check_size = 8`，`(0,0)` 深色。
    #[test]
    fn checker_matches_measured_gdkpixbuf() {
        assert_eq!(checker_size(32), 8);
        assert_eq!(checker_size(24), 4, "24/6 = 4 → 2 → 4");
        assert_eq!(checker_size(256), 16, "上限 16");
        assert_eq!(checker_color(0, 0, 8), CHECKER_DARK);
        assert_eq!(checker_color(8, 0, 8), CHECKER_LIGHT);
        assert_eq!(checker_color(0, 8, 8), CHECKER_LIGHT);
        assert_eq!(checker_color(8, 8, 8), CHECKER_DARK);
        assert_eq!(CHECKER_DARK, [112, 112, 112], "0xFF707070 是 AARRGGBB");
        assert_eq!(CHECKER_LIGHT, [128, 128, 128]);
    }

    /// 缩放几何：缩略图最长边铺满方格，另一边居中留边。
    #[test]
    fn preview_geometry_fits_and_centers() {
        assert_eq!(preview_geometry(64, 64, 32), (0, 0, 32, 32));
        // 256×188 → scale 0.125 → 32×23（截断），竖直居中 y = 4
        assert_eq!(preview_geometry(256, 188, 32), (0, 4, 32, 23));
        // 16×16 → 放大到 32×32（make_preview 的缩放不是 shrink_only）
        assert_eq!(preview_geometry(16, 16, 32), (0, 0, 32, 32));
    }

    /// 预览格：棋盘打底、缩略图居中覆盖。
    #[test]
    fn preview_composites_thumbnail_over_checker() {
        // 256×188 纯红缩略图 → 上下留出棋盘边
        let mut src = vec![0u8; 256 * 188 * 4];
        for p in src.chunks_exact_mut(4) {
            p.copy_from_slice(&[255, 0, 0, 255]);
        }
        let out = compose_preview(&src, 256, 188, 32);
        assert_eq!(out.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| {
            let o = (y * 32 + x) * 4;
            [out[o], out[o + 1], out[o + 2], out[o + 3]]
        };
        assert_eq!(at(16, 16), [255, 0, 0, 255], "正中是缩略图");
        assert_eq!(at(0, 0), [112, 112, 112, 255], "左上角是棋盘深色");
        assert_eq!(at(0, 31), [128, 128, 128, 255], "棋盘奇偶正确");
    }

    /// 全透明缩略图（空层）→ 只剩棋盘。
    #[test]
    fn preview_of_empty_thumbnail_is_pure_checker() {
        let src = vec![0u8; 64 * 64 * 4];
        let out = compose_preview(&src, 64, 64, 32);
        for (i, c) in out.chunks_exact(4).enumerate() {
            let (x, y) = (i % 32, i / 32);
            let expected = checker_color(x as i32, y as i32, 8);
            assert_eq!(&c[0..3], &expected[..]);
            assert_eq!(c[3], 255);
        }
    }

    /// `render_halved` 的输出尺寸与 rect/level 的关系（`>>` 对负数是向下取整）。
    #[test]
    fn render_halved_sizes_and_offsets() {
        let (px, w, h) = render_halved((-64, -64, 64, 64), 1, encode_alpha, |_, _| [0; 4]);
        assert_eq!((w, h), (32, 32));
        assert_eq!(px.len(), 32 * 32 * 4);
        assert!(px.iter().all(|b| *b == 0), "全零 → 全透明");
    }
}
