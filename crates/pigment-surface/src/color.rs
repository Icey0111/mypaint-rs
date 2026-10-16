//! fix15（预乘 alpha、线性）tile → 8 位显示像素的转换。
//!
//! 这段逻辑原先在 `pigment-app/src/convert.rs`（GPU 上传用）。文档 I/O 也要
//! 用它（导出 PNG / ORA），而 `pigment-app` 是应用层、`pigment-engine` 是引擎层，
//! 引擎不该依赖应用层——所以按 MyPaint 的分层把它下沉到这里：源码里
//! `tile_convert_rgba16_to_rgba8`（`lib/pixops.cpp:227`）与 `save_as_png`
//! （`lib/surface.py:220`）都在 `lib/`，本 crate 对应的正是这一层。
//! `pigment-app::convert` 现在只是本模块的转发。
//!
//! ## 色彩空间：MyPaint 用的是 EOTF 2.2，不是 IEC sRGB 曲线
//!
//! 实证：`lib/pixops.cpp:315` 的 `tile_convert_rgba16_to_rgba8` 走的是
//! `fastpow(c, 1.0/EOTF)`，而 `EOTF` 来自 `lib/eotf.py`：`DEFAULT_EOTF = 2.2`，
//! 且 `gui/compatibility.py:460-463` 只在用户显式切到 **1.x 兼容模式** 时才
//! `set_eotf(1.0)`，2.x（默认）用 `base_eotf()` = 2.2。
//!
//! 所以正确的转换是 `pow(c, 1/2.2)`，而**不是**分段 sRGB 曲线
//! （`1.055 * c^(1/2.4) - 0.055`）。两者在中灰处差 ~1.5/255：sRGB(0.5)=0.7354，
//! 2.2(0.5)=0.7297 —— 肉眼几乎看不出，但这是实打实的差异，故按源码走 2.2。
//!
//! ## 画布底色
//!
//! 底色取自 MyPaint 的默认背景：`backgrounds/mrmamurk/mamurk_e_1.png`
//! （`gui/backgroundwindow.py` 的 `FALLBACK_BACKGROUND`），平铺后的平均色。
//! 实测 MyPaint 实机画布屏幕采样为 `(238, 240, 227)`，与该均值
//! `(237, 239, 227)` 一致——它不是纯白，这是「和 MyPaint 对不上」的可见项之一。

// 逐像素循环刻意用 `for y in 0..TILE_SIZE { for x in ... }` 并直接索引
// `pixels[y][x]`：下标**就是**坐标，与源码 `lib/pixops.cpp` 的写法一一对应。
// 改成 `enumerate()` 不会更好读，反而看不出 (x, y) 与 tile 内偏移的关系。
#![allow(clippy::needless_range_loop)]

use hokusai::fix15;
use hokusai::tile::{TilePixels, TILE_SIZE};

/// 单块 tile 转成 RGBA8 后的字节数。
pub const TILE_RGBA8_BYTES: usize = TILE_SIZE * TILE_SIZE * 4;

/// MyPaint 默认画布底色（米白纸色），sRGB8。
pub const CANVAS_BG: [u8; 3] = [0xED, 0xEF, 0xE3];

/// MyPaint 默认 EOTF —— `lib/eotf.py:DEFAULT_EOTF`。
pub const EOTF: f32 = 2.2;

/// 线性值 → 显示值 —— 照抄 `lib/pixops.cpp:315` 的 `fastpow(c, 1.0/EOTF)`。
///
/// 注意：**不是** IEC sRGB 曲线。见模块头注释。
pub fn eotf_encode(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / EOTF)
}

/// 显示值 → 线性值 —— `lib/pixops.cpp:539` 的 `fastpow(c/255, EOTF)`。
pub fn eotf_decode(c: f32) -> f32 {
    c.max(0.0).powf(EOTF)
}

/// 8 位显示通道 → 线性浮点。
#[inline]
fn bg_linear(bg: [u8; 3]) -> [f32; 3] {
    [
        eotf_decode(bg[0] as f32 / 255.0),
        eotf_decode(bg[1] as f32 / 255.0),
        eotf_decode(bg[2] as f32 / 255.0),
    ]
}

/// 单个像素：fix15 预乘线性 → 去预乘 + 合成底色 + EOTF 编码，写 3 字节。
///
/// 逐像素照抄 `tile_convert_rgba16_to_rgba8`（`lib/pixops.cpp:227`）：
/// 去预乘（带舍入）→ `pow(c, 1/EOTF) * 255 + 0.5`。
#[inline]
fn encode_pixel_composited(p: [u16; 4], bg: [u8; 3], bg_lin: [f32; 3], out: &mut [u8]) {
    let a = fix15::to_f32(p[3]);
    if a > 0.0 {
        let r = fix15::to_f32(p[0]) / a;
        let g = fix15::to_f32(p[1]) / a;
        let b = fix15::to_f32(p[2]) / a;
        out[0] = (eotf_encode((r * a + bg_lin[0] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
        out[1] = (eotf_encode((g * a + bg_lin[1] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
        out[2] = (eotf_encode((b * a + bg_lin[2] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
    } else {
        out[0] = bg[0];
        out[1] = bg[1];
        out[2] = bg[2];
    }
}

/// 单个像素：fix15 预乘线性 → 去预乘的直通 alpha，写 4 字节。
///
/// 对应源码里 `blit_tile_into(dst, dst_has_alpha=True, ...)` 走
/// `tile_convert_rgba16_to_rgba8` 的路径；导出的 PNG 保留透明。
#[inline]
fn encode_pixel_with_alpha(p: [u16; 4], out: &mut [u8]) {
    let a = fix15::to_f32(p[3]);
    out[3] = (a * 255.0).round() as u8;
    if a > 0.0 {
        let r = fix15::to_f32(p[0]) / a;
        let g = fix15::to_f32(p[1]) / a;
        let b = fix15::to_f32(p[2]) / a;
        out[0] = (eotf_encode(r.clamp(0.0, 1.0)) * 255.0).round() as u8;
        out[1] = (eotf_encode(g.clamp(0.0, 1.0)) * 255.0).round() as u8;
        out[2] = (eotf_encode(b.clamp(0.0, 1.0)) * 255.0).round() as u8;
    } else {
        out[0] = 0;
        out[1] = 0;
        out[2] = 0;
    }
}

/// 把一块 fix15 tile 合成到 `bg` 底色并转为显示用 RGBA8。
///
/// 与源码的两点差别，都记在案：
/// - 省掉了 `dithering_noise`（固定伪随机抖动，幅度约 ±2%，肉眼不可见）；
/// - 空像素直接写底色（MyPaint 是合成到背景层，结果等价）。
pub fn tile_to_rgba8(pixels: &TilePixels, bg: [u8; 3]) -> Box<[u8; TILE_RGBA8_BYTES]> {
    // 底色转线性域：合成必须在线性空间做，否则半透明边缘会偏亮
    let bg_lin = bg_linear(bg);
    let mut out = Box::new([0u8; TILE_RGBA8_BYTES]);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let i = (y * TILE_SIZE + x) * 4;
            encode_pixel_composited(pixels[y][x], bg, bg_lin, &mut out[i..i + 3]);
            out[i + 3] = 255;
        }
    }
    out
}

/// 把一块 fix15 tile 转为**带真实 alpha** 的 RGBA8（不合成底色）。
///
/// ORA 的图层 PNG 走这条（透明必须保留）。
pub fn tile_to_rgba8_alpha(pixels: &TilePixels) -> Box<[u8; TILE_RGBA8_BYTES]> {
    let mut out = Box::new([0u8; TILE_RGBA8_BYTES]);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let i = (y * TILE_SIZE + x) * 4;
            encode_pixel_with_alpha(pixels[y][x], &mut out[i..i + 4]);
        }
    }
    out
}

/// 单块 tile：按 `alpha` 选择输出形式（合成底色 / 直通 alpha）。
pub fn tile_to_rgba8_mode(
    pixels: &TilePixels,
    bg: [u8; 3],
    alpha: bool,
) -> Box<[u8; TILE_RGBA8_BYTES]> {
    if alpha {
        tile_to_rgba8_alpha(pixels)
    } else {
        tile_to_rgba8(pixels, bg)
    }
}

/// 单个像素：合成底色，返回 RGBA8（alpha 恒 255）。
///
/// 供矩形渲染（[`crate::PigmentSurface::render_rect_rgba8`]）逐像素调用。
#[inline]
pub fn tile_to_rgba8_single(p: [u16; 4], bg: [u8; 3], bg_lin: [f32; 3]) -> [u8; 4] {
    let mut out = [0u8; 4];
    encode_pixel_composited(p, bg, bg_lin, &mut out[..3]);
    out[3] = 255;
    out
}

/// 单个像素：直通 alpha，返回 RGBA8。
#[inline]
pub fn tile_to_rgba8_alpha_single(p: [u16; 4]) -> [u8; 4] {
    let mut out = [0u8; 4];
    encode_pixel_with_alpha(p, &mut out);
    out
}

/// 把 8 位底色转为线性域（矩形渲染前调一次，避免逐像素重复解码）。
#[inline]
pub fn bg_to_linear(bg: [u8; 3]) -> [f32; 3] {
    bg_linear(bg)
}

/// 8 位显示像素 → fix15 预乘线性（导入 PNG 用）。
///
/// 是 [`encode_pixel_with_alpha`] 的逆：`pow(c/255, EOTF)` 得线性值，
/// 再乘 alpha 变预乘，最后定点化。对应 `lib/pixops.cpp` 的
/// `tile_convert_rgba8_to_rgba16`。
#[inline]
pub fn rgba8_to_fix15(r: u8, g: u8, b: u8, a: u8) -> [u16; 4] {
    let af = a as f32 / 255.0;
    let lin = |c: u8| eotf_decode(c as f32 / 255.0);
    [
        fix15::from_f32(lin(r) * af),
        fix15::from_f32(lin(g) * af),
        fix15::from_f32(lin(b) * af),
        fix15::from_f32(af),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::tile::empty_tile;

    const WHITE: [u8; 3] = [255, 255, 255];

    #[test]
    fn empty_tile_is_background() {
        let t = empty_tile();
        let img = tile_to_rgba8(&t, CANVAS_BG);
        assert!(img.chunks(4).all(|p| p[..3] == CANVAS_BG && p[3] == 255));
    }

    #[test]
    fn empty_tile_on_white_is_white() {
        let t = empty_tile();
        let img = tile_to_rgba8(&t, WHITE);
        assert!(img.iter().all(|&b| b == 255));
    }

    #[test]
    fn opaque_black_is_black() {
        let mut t = empty_tile();
        // fix15 满 alpha = 32768（1<<15），RGB 通道为 0（黑）
        t[10][20] = [0, 0, 0, 1 << 15];
        let img = tile_to_rgba8(&t, CANVAS_BG);
        let i = (10 * TILE_SIZE + 20) * 4;
        assert_eq!(&img[i..i + 3], &[0, 0, 0]);
        assert_eq!(img[i + 3], 255);
        // 其余像素仍为底色
        assert_eq!(&img[0..3], &CANVAS_BG);
    }

    #[test]
    fn half_alpha_linear_white_is_midpoint_on_white() {
        let mut t = empty_tile();
        // 半透明白：线性值 0.5（预乘：0.5 * alpha）
        let a = (1 << 14) as u16; // alpha = 0.5
        let half = ((1u32 << 15) as f32 * 0.25) as u16; // 预乘后通道 = 0.5*0.5
        t[0][0] = [half, half, half, a];
        let img = tile_to_rgba8(&t, WHITE);
        // 去预乘得 r=0.5，合成白底 = 0.5*0.5 + 1.0*0.5 = 0.75（线性）→ sRGB≈225
        let v = img[0];
        assert!(
            (218..=232).contains(&v),
            "半透明白合成后应为 sRGB≈225，实际 {v}"
        );
    }

    /// 半透明黑落在米白底上：结果必须比底色暗、比纯黑亮。
    #[test]
    fn half_alpha_black_darkens_background() {
        let mut t = empty_tile();
        let a = (1 << 14) as u16; // alpha = 0.5
        t[0][0] = [0, 0, 0, a];
        let img = tile_to_rgba8(&t, CANVAS_BG);
        let v = img[0];
        assert!(
            v < CANVAS_BG[0] && v > 0,
            "半透明黑应介于底色 {} 与 0 之间，实际 {v}",
            CANVAS_BG[0]
        );
    }

    /// EOTF 圆整：`eotf_decode` 与 `eotf_encode` 互逆，且 0/1 是定点。
    #[test]
    fn eotf_round_trips() {
        assert_eq!(eotf_encode(0.0), 0.0);
        assert_eq!(eotf_encode(1.0), 1.0);
        for v in [0.05f32, 0.25, 0.5, 0.75, 0.99] {
            let back = eotf_encode(eotf_decode(v));
            assert!((back - v).abs() < 1e-5, "{v} 往返失败：{back}");
        }
    }

    /// 关键实证：MyPaint 走 `pow(c, 1/2.2)`，**不是**分段 IEC sRGB 曲线。
    ///
    /// 若哪天有人把它换回 `linear_to_srgb`，这条测试会挂 —— 中灰处两者差约
    /// 1.5/255（2.2 编码 0.5 → 186.1；sRGB 编码 0.5 → 187.5）。
    #[test]
    fn eotf_is_gamma_22_not_iec_srgb() {
        assert_eq!(EOTF, 2.2);
        let v = (eotf_encode(0.5) * 255.0).round() as i32;
        assert_eq!(v, 186, "0.5 线性应编码为 186（pow(0.5,1/2.2)），实际 {v}");
        // 作为对照：hokusai 的分段 sRGB 曲线会给出 188 左右，绝不能相等
        let iec = (hokusai::color::linear_to_srgb(0.5) * 255.0).round() as i32;
        assert_ne!(v, iec, "EOTF 与 IEC sRGB 曲线不应给出同一结果");
    }

    /// 底色在往返后必须逐字节不变（否则整个画布会整体偏色）。
    #[test]
    fn canvas_bg_round_trips_exactly() {
        let back = [
            (eotf_encode(eotf_decode(CANVAS_BG[0] as f32 / 255.0)) * 255.0).round() as u8,
            (eotf_encode(eotf_decode(CANVAS_BG[1] as f32 / 255.0)) * 255.0).round() as u8,
            (eotf_encode(eotf_decode(CANVAS_BG[2] as f32 / 255.0)) * 255.0).round() as u8,
        ];
        assert_eq!(back, CANVAS_BG);
    }

    /// 直通 alpha 模式：空像素是全透明黑，不合成底色。
    #[test]
    fn alpha_mode_keeps_transparency() {
        let t = empty_tile();
        let img = tile_to_rgba8_alpha(&t);
        assert_eq!(&img[0..4], &[0, 0, 0, 0], "空 tile 应全透明");
    }

    /// 满 alpha 的纯色在两种模式下 RGB 一致（alpha 只影响合成结果）。
    #[test]
    fn opaque_pixel_same_rgb_in_both_modes() {
        let mut t = empty_tile();
        t[1][1] = [1 << 14, 1 << 15, 1 << 13, 1 << 15]; // 不透明 (0.5, 1.0, 0.25)
        let comp = tile_to_rgba8(&t, CANVAS_BG);
        let alpha = tile_to_rgba8_alpha(&t);
        let i = (TILE_SIZE + 1) * 4;
        assert_eq!(&comp[i..i + 3], &alpha[i..i + 3], "不透明像素两模式 RGB 应相同");
        assert_eq!(alpha[i + 3], 255);
    }

    /// `rgba8_to_fix15` 与 `encode_pixel_with_alpha` 大体互逆。
    #[test]
    fn rgba8_to_fix15_round_trips_within_quantisation() {
        for (r, g, b, a) in [
            (255u8, 0u8, 0u8, 255u8),
            (0, 255, 0, 255),
            (128, 128, 128, 255),
            (10, 200, 30, 128),
        ] {
            let px = rgba8_to_fix15(r, g, b, a);
            let mut out = [0u8; 4];
            encode_pixel_with_alpha(px, &mut out);
            assert!(
                (out[0] as i32 - r as i32).abs() <= 1
                    && (out[1] as i32 - g as i32).abs() <= 1
                    && (out[2] as i32 - b as i32).abs() <= 1
                    && (out[3] as i32 - a as i32).abs() <= 1,
                "({r},{g},{b},{a}) 往返成 {out:?}"
            );
        }
    }
}
