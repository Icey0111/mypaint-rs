//! 大半径 smudge 的并行取色。
//!
//! `get_color_pigment_default` 每个 dab 要遍历半径内的整片像素。半径 1000 时
//! 单次调用约 450 万像素、8ms 左右，而且它跑在绘制热路径上。这里按 tile
//! 并行执行同样的 mask 遍历，并**严格保留**：
//!
//! - 每个 tile 独立的 interval 计数器（从 0 开始）；
//! - 全局 libc-rand 序列（用 LCG 跳进算法为每个 tile 预置起始状态）；
//! - 采样点的全局顺序（按 tile 行主序收集，再串行累加 WGM）。
//!
//! 因此结果与标量实现逐位一致，见同文件的对照测试。

use hokusai::brushmodes::{
    color_rand_step, color_rng_advance, color_rng_state, for_each_mask_pixel,
    set_color_rng_state, COLOR_RAND_MAX,
};
use hokusai::color::RgbaF32;
use hokusai::fix15::FIX15_ONE;
use hokusai::spectral::{fastpow, rgb_to_spectral, spectral_to_rgb};
use hokusai::tile::{TilePixels, TILE_SIZE};
use rayon::prelude::*;

use crate::symmetry::dab_pool;
use crate::PigmentSurface;

/// 覆盖像素数：mask 非零的像素总数（`for_each_mask_pixel` 已跳过零）。
fn count_covered(x: f32, y: f32, radius: f32, tx: i32, ty: i32) -> usize {
    let mut n = 0usize;
    for_each_mask_pixel(x, y, radius, 0.5, tx, ty, |_, _, _| n += 1);
    n
}

/// 并行版 `get_color_pigment_default`；不满足加速条件时返回 `None`。
#[cfg(target_os = "windows")]
pub(crate) fn get_color_pigment_parallel(
    surface: &PigmentSurface,
    x: f32,
    y: f32,
    radius: f32,
    paint: f32,
) -> Option<RgbaF32> {
    if !(paint >= 0.0) {
        return None;
    }
    let radius = if radius < 1.0 { 1.0 } else { radius };
    let ts = TILE_SIZE as i32;
    let r_fringe = radius + 1.0;
    let tx0 = ((x - r_fringe).floor() as i32).div_euclid(ts);
    let tx1 = ((x + r_fringe).floor() as i32).div_euclid(ts);
    let ty0 = ((y - r_fringe).floor() as i32).div_euclid(ts);
    let ty1 = ((y + r_fringe).floor() as i32).div_euclid(ts);

    let mut keys: Vec<(i32, i32)> = Vec::new();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            keys.push((tx, ty));
        }
    }
    if keys.len() < 4 {
        return None;
    }

    let sample_interval: u32 = if radius <= 2.0 {
        1
    } else {
        (radius * 7.0) as u32
    };
    let random_sample_rate = 1.0_f32 / (7.0 * radius);
    let random_sample_threshold = (random_sample_rate * COLOR_RAND_MAX as f32) as i32;

    let pool = dab_pool();
    let covered: Vec<usize> = pool.install(|| {
        keys.par_iter()
            .map(|&(tx, ty)| count_covered(x, y, radius, tx, ty))
            .collect()
    });

    // 每个 tile 的 interval 采样次数只由覆盖像素数决定；其余像素都会消耗
    // 一次 RNG。据此用 LCG 跳进为每个 tile 算出起始状态。
    let mut starts: Vec<u64> = Vec::with_capacity(keys.len());
    let mut state = color_rng_state();
    let interval = sample_interval as usize;
    for &count in &covered {
        starts.push(state);
        let interval_samples = if count == 0 {
            0
        } else {
            1 + (count - 1) / interval.max(1)
        };
        let draws = count.saturating_sub(interval_samples) as u64;
        state = color_rng_advance(state, draws);
    }
    set_color_rng_state(state);

    let tiles: Vec<Option<&TilePixels>> =
        keys.iter().map(|&(tx, ty)| surface.tile(tx, ty)).collect();
    let per_tile: Vec<Vec<(u32, [u16; 4])>> = pool.install(|| {
        keys.par_iter()
            .zip(tiles.par_iter())
            .zip(starts.par_iter())
            .map(|((&(tx, ty), &tile), &start)| {
                let mut samples: Vec<(u32, [u16; 4])> = Vec::new();
                let mut counter = 0u32;
                let mut rng = start;
                for_each_mask_pixel(x, y, radius, 0.5, tx, ty, |lx, ly, mask| {
                    let sampled = counter == 0
                        || (sample_interval > 1
                            && color_rand_step(&mut rng) < random_sample_threshold);
                    if sampled {
                        let p = tile.map(|t| t[ly][lx]).unwrap_or([0, 0, 0, 0]);
                        samples.push((mask, p));
                    }
                    counter += 1;
                    if counter == sample_interval {
                        counter = 0;
                    }
                });
                samples
            })
            .collect()
    });

    let mut sum_weight = 0.0_f32;
    let mut sum_a = 0.0_f32;
    // Reference boundary: libmypaint calls the accumulator once per tile and
    // re-derives the spectral average from the previous tile's `sum_r/g/b`
    // (see the comment in `get_color_pigment_default`). Mirror that here so
    // the parallel path stays bit-identical to the scalar one for sample
    // radii spanning multiple tiles.
    let mut sum_r = 0.0_f32;
    let mut sum_g = 0.0_f32;
    let mut sum_b = 0.0_f32;
    for samples in &per_tile {
        let mut avg_spectral = if paint > 0.0 {
            rgb_to_spectral(sum_r, sum_g, sum_b)
        } else {
            [0.0_f32; 10]
        };
        let mut avg_rgb = [sum_r, sum_g, sum_b];
        for &(mask, p) in samples {
            let pa = p[3] as u32;
            let a = mask as f32 * pa as f32 / (1u32 << 30) as f32;
            let alpha_sums = a + sum_a;
            sum_weight += mask as f32 / FIX15_ONE as f32;
            let (mut fac_a, mut fac_b) = (1.0_f32, 1.0_f32);
            if alpha_sums > 0.0 {
                fac_a = a / alpha_sums;
                fac_b = 1.0 - fac_a;
            }
            if paint > 0.0 && pa > 0 {
                let spectral = rgb_to_spectral(
                    p[0] as f32 / pa as f32,
                    p[1] as f32 / pa as f32,
                    p[2] as f32 / pa as f32,
                );
                for i in 0..10 {
                    avg_spectral[i] =
                        fastpow(spectral[i], fac_a) * fastpow(avg_spectral[i], fac_b);
                }
            }
            if paint < 1.0 && pa > 0 {
                for i in 0..3 {
                    avg_rgb[i] = p[i] as f32 * fac_a / pa as f32 + avg_rgb[i] * fac_b;
                }
            }
            sum_a += a;
        }
        let spec_rgb = spectral_to_rgb(&avg_spectral);
        sum_r = spec_rgb.0 * paint + (1.0 - paint) * avg_rgb[0];
        sum_g = spec_rgb.1 * paint + (1.0 - paint) * avg_rgb[1];
        sum_b = spec_rgb.2 * paint + (1.0 - paint) * avg_rgb[2];
    }

    if sum_weight <= 0.0 {
        return Some(RgbaF32::TRANSPARENT);
    }
    sum_a /= sum_weight;
    let a_out = sum_a.clamp(0.0, 1.0);
    if sum_a > 0.0 {
        Some(RgbaF32 {
            r: sum_r.clamp(0.0, 1.0),
            g: sum_g.clamp(0.0, 1.0),
            b: sum_b.clamp(0.0, 1.0),
            a: a_out,
        })
    } else {
        Some(RgbaF32 {
            r: 0.0,
            g: 1.0,
            b: 0.0,
            a: a_out,
        })
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn get_color_pigment_parallel(
    _surface: &PigmentSurface,
    _x: f32,
    _y: f32,
    _radius: f32,
    _paint: f32,
) -> Option<RgbaF32> {
    None
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use hokusai::brushmodes::{color_rng_state, get_color_pigment_default, reset_get_color_rng};
    use hokusai::TiledSurface;

    /// 并行取色必须与标量逐位一致，并且 RNG 终态相同。
    #[test]
    fn parallel_pigment_sampling_matches_scalar_bitwise() {
        let mut s = PigmentSurface::new();
        for ty in -8..=8 {
            for tx in -8..=8 {
                let t = s.tile_request_start(tx, ty);
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        let a = if (tx + ty).rem_euclid(3) == 0 {
                            0
                        } else {
                            (1 << 15) as u16
                        };
                        t[y][x] = [
                            (((tx + 8) as u16 * 1234 + x as u16 * 37 + y as u16 * 11)
                                % 30000) as u16,
                            (((ty + 8) as u16 * 777 + x as u16 * 53 + y as u16 * 7)
                                % 30000) as u16,
                            (((tx + ty + 16) as u16 * 401 + x as u16 * 3 + y as u16 * 97)
                                % 30000) as u16,
                            a,
                        ];
                    }
                }
                s.tile_request_end(tx, ty);
            }
        }

        for radius in [32.0_f32, 64.0, 128.0, 400.0] {
            for &paint in &[0.0_f32, 0.25, 0.6, 1.0] {
                reset_get_color_rng();
                let want = get_color_pigment_default(&s, 32.5, 40.5, radius, paint);
                let want_state = color_rng_state();

                reset_get_color_rng();
                let got = get_color_pigment_parallel(&s, 32.5, 40.5, radius, paint)
                    .expect("并行路径应可用");
                let got_state = color_rng_state();

                assert_eq!(want.r.to_bits(), got.r.to_bits(), "r radius={radius} paint={paint}");
                assert_eq!(want.g.to_bits(), got.g.to_bits(), "g radius={radius} paint={paint}");
                assert_eq!(want.b.to_bits(), got.b.to_bits(), "b radius={radius} paint={paint}");
                assert_eq!(want.a.to_bits(), got.a.to_bits(), "a radius={radius} paint={paint}");
                assert_eq!(want_state, got_state, "RNG radius={radius} paint={paint}");
            }
        }
    }
}
