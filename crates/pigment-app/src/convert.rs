//! fix15（预乘 alpha、线性）tile → 显示用 RGBA8 转换。
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
//! M2 采用「上传前在 CPU 侧合成到画布底色」策略：纹理存合成后的不透明 sRGB8，
//! GPU shader 因此只剩一次采样。tile 只有 64×64=4096 像素，转换开销可忽略；
//! 真正需要 alpha 语义（图层/导出）时再改回 Rgba16Float 管线。
//!
//! ## 画布底色
//!
//! 底色取自 MyPaint 的默认背景：`backgrounds/mrmamurk/mamurk_e_1.png`
//! （`gui/backgroundwindow.py` 的 `FALLBACK_BACKGROUND`），平铺后的平均色。
//! 实测 MyPaint 实机画布屏幕采样为 `(238, 240, 227)`，与该均值
//! `(237, 239, 227)` 一致——它不是纯白，这是「和 MyPaint 对不上」的可见项之一。

use pigment_engine::hokusai::fix15;
use pigment_engine::hokusai::tile::{TilePixels, TILE_SIZE};

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

/// [`CANVAS_BG`] 的 egui 形式。
pub fn canvas_bg_color() -> eframe::egui::Color32 {
    eframe::egui::Color32::from_rgb(CANVAS_BG[0], CANVAS_BG[1], CANVAS_BG[2])
}

/// 把一块 fix15 tile 合成到 `bg` 底色并转为显示用 RGBA8。
///
/// 逐像素照抄 `tile_convert_rgba16_to_rgba8`（`lib/pixops.cpp:227`）：
/// 去预乘（带舍入）→ `pow(c, 1/EOTF) * 255 + 0.5`。
///
/// 与源码的两点差别，都记在案：
/// - 省掉了 `dithering_noise`（固定伪随机抖动，幅度约 ±2%，肉眼不可见）；
/// - 空像素直接写底色（MyPaint 是合成到背景层，结果等价）。
pub fn tile_to_rgba8(pixels: &TilePixels, bg: [u8; 3]) -> Box<[u8; TILE_RGBA8_BYTES]> {
    // 底色转线性域：合成必须在线性空间做，否则半透明边缘会偏亮
    let bg_lin = [
        eotf_decode(bg[0] as f32 / 255.0),
        eotf_decode(bg[1] as f32 / 255.0),
        eotf_decode(bg[2] as f32 / 255.0),
    ];
    let mut out = Box::new([0u8; TILE_RGBA8_BYTES]);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let p = pixels[y][x];
            let a = fix15::to_f32(p[3]);
            let i = (y * TILE_SIZE + x) * 4;
            if a > 0.0 {
                // 去预乘 → 合成到底色（线性域）→ 转显示
                let r = fix15::to_f32(p[0]) / a;
                let g = fix15::to_f32(p[1]) / a;
                let b = fix15::to_f32(p[2]) / a;
                out[i] =
                    (eotf_encode((r * a + bg_lin[0] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round()
                        as u8;
                out[i + 1] = (eotf_encode((g * a + bg_lin[1] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0)
                    .round() as u8;
                out[i + 2] = (eotf_encode((b * a + bg_lin[2] * (1.0 - a)).clamp(0.0, 1.0)) * 255.0)
                    .round() as u8;
            } else {
                // 空像素 = 底色
                out[i] = bg[0];
                out[i + 1] = bg[1];
                out[i + 2] = bg[2];
            }
            out[i + 3] = 255;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::hokusai::tile::empty_tile;

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
        let iec = (pigment_engine::hokusai::color::linear_to_srgb(0.5) * 255.0).round() as i32;
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
}
