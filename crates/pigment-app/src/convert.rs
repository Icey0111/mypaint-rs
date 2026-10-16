//! fix15（预乘 alpha、线性 sRGB）tile → 显示用 RGBA8（sRGB）转换。
//!
//! M2 采用「上传前在 CPU 侧合成到白纸底」策略：纹理存合成后的不透明 sRGB8，
//! GPU shader 因此只剩一次采样。tile 只有 64×64=4096 像素，转换开销可忽略；
//! 真正需要 alpha 语义（图层/导出）时再改回 Rgba16Float 管线。

use pigment_engine::hokusai::color::linear_to_srgb;
use pigment_engine::hokusai::fix15;
use pigment_engine::hokusai::tile::{TilePixels, TILE_SIZE};

pub const TILE_RGBA8_BYTES: usize = TILE_SIZE * TILE_SIZE * 4;

/// 把一块 fix15 tile 合成到白色纸底并转为 sRGB8。
pub fn tile_to_rgba8_on_white(pixels: &TilePixels) -> Box<[u8; TILE_RGBA8_BYTES]> {
    let mut out = Box::new([255u8; TILE_RGBA8_BYTES]);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let p = pixels[y][x];
            let a = fix15::to_f32(p[3]);
            let i = (y * TILE_SIZE + x) * 4;
            if a > 0.0 {
                // 去预乘 → 合成到白底 → 线性转 sRGB
                let r = fix15::to_f32(p[0]) / a;
                let g = fix15::to_f32(p[1]) / a;
                let b = fix15::to_f32(p[2]) / a;
                out[i] = (linear_to_srgb((r * a + (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
                out[i + 1] =
                    (linear_to_srgb((g * a + (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
                out[i + 2] =
                    (linear_to_srgb((b * a + (1.0 - a)).clamp(0.0, 1.0)) * 255.0).round() as u8;
            }
            // a <= 0：保持初始化的白色
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::hokusai::tile::empty_tile;

    #[test]
    fn empty_tile_is_white() {
        let t = empty_tile();
        let img = tile_to_rgba8_on_white(&t);
        assert!(img.iter().all(|&b| b == 255));
    }

    #[test]
    fn opaque_black_is_black() {
        let mut t = empty_tile();
        // fix15 满 alpha = 32768（1<<15），RGB 通道为 0（黑）
        t[10][20] = [0, 0, 0, 1 << 15];
        let img = tile_to_rgba8_on_white(&t);
        let i = (10 * TILE_SIZE + 20) * 4;
        assert_eq!(&img[i..i + 3], &[0, 0, 0]);
        assert_eq!(img[i + 3], 255);
        // 其余像素仍为白
        assert_eq!(&img[0..3], &[255, 255, 255]);
    }

    #[test]
    fn half_alpha_linear_white_is_midpoint() {
        let mut t = empty_tile();
        // 半透明白：线性值 0.5（预乘：0.5 * alpha）
        let a = (1 << 14) as u16; // alpha = 0.5
        let half = ((1u32 << 15) as f32 * 0.25) as u16; // 预乘后通道 = 0.5*0.5
        t[0][0] = [half, half, half, a];
        let img = tile_to_rgba8_on_white(&t);
        // 合成到白底：0.5*1 + 0.5*1 = 线性 1.0 → 255？不对——去预乘得 r=0.5，
        // 合成 = 0.5*0.5 + 0.5 = 0.75（线性），sRGB(0.75)*255 ≈ 225
        let v = img[0];
        assert!(
            (218..=232).contains(&v),
            "半透明白合成后应为 sRGB≈225，实际 {v}"
        );
    }
}
