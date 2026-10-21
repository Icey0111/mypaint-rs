//! 笔刷缩略图 —— 照搬 MyPaint `gui/drawutils.py` 的预览渲染。
//!
//! 源码对照：
//!
//! | 本文件 | MyPaint |
//! |---|---|
//! | [`PREVIEW_POINTS`] | `_BRUSH_PREVIEW_POINTS`（`drawutils.py:38`） |
//! | [`spline_4p`] / [`spline_iter`] | `drawutils.py:60` / `:89`（Catmull-Rom） |
//! | [`variable_pressure_scribble`] | `drawutils.py:118` |
//! | [`brush_preview_bg_fg`] | `drawutils.py:203` |
//! | [`render`] | `render_brush_preview_pixbuf`（`drawutils.py:135`） |
//! | [`tile_to_rgba8`] | `mypaintlib.tile_convert_rgba16_to_rgba8`（`lib/pixops.cpp:227`） |
//!
//! 三处**刻意保留**的 MyPaint 行为（都不是"设计取舍"）：
//!
//! 1. **画布随笔画增长**：`size_in_tiles` 从 1 试到 `max_edge_tiles - 1`，笔画
//!    溢出到界外就换更大的画布重画（`drawutils.py:158`）。所以同一支笔在
//!    64×64 与 192×192 两档之间会得到**不同取景**，与 MyPaint 一致。
//! 2. **对角双色底**：底色由笔刷性格决定 —— 橡皮是粉红、colorize 是橙、smudge 偏蓝，
//!    其余是纸黄灰（`drawutils.py:206`）。底色本身也参与混色（smudge 类笔刷会与它涂抹）。
//! 3. **叠棋盘格**：最终图叠在 16px 灰棋盘上输出（`drawutils.py:191`），
//!    所以橡皮/擦除类笔刷的"擦除"在缩略图里可见。
//!
//! 一处**技术栈适配**：MyPaint 的 `pixops.cpp` 在 8bit 输出时叠加了
//! `dithering_noise`（固定伪随机序列，幅度约 ±2%）。复刻它需要连同 `rand()` 的
//! 调用序一起复刻，肉眼不可见，故不加噪声，只保留
//!「去预乘（带舍入）→ `pow(x, 1/EOTF)` → 四舍五入」的算式。
//!
//! **注意**：只有取不到随附的 `{name}_prev.png` 时才走这里。官方包 196 支全部
//! 自带预览图，所以正常运行下本模块只服务用户自制笔刷 —— 与 MyPaint 的
//! `ManagedBrush.get_preview()`（`gui/brushmanager.py:1242`）行为一致。

use pigment_engine::hokusai::fix15;
use pigment_engine::hokusai::tile::{TilePixels, TILE_SIZE};
use pigment_engine::hokusai::{Brush, BrushSetting, BrushState, TiledSurface};
use pigment_surface::PigmentSurface;

/// 预览图边长 —— `PREVIEW_W` / `PREVIEW_H`（`gui/brushmanager.py:51`）。
pub const PREVIEW_W: usize = 128;
/// 预览图边长（同 [`PREVIEW_W`]）。
pub const PREVIEW_H: usize = 128;

/// `render_brush_preview_pixbuf(max_edge_tiles=4)`：画布最大 3×3 个 tile。
const MAX_EDGE_TILES: usize = 4;

/// 棋盘格尺寸/颜色 —— `gui/style.py:ALPHA_CHECK_*`。
const ALPHA_CHECK_SIZE: usize = 16;
const ALPHA_CHECK_COLOR_1: f32 = 0.45;
const ALPHA_CHECK_COLOR_2: f32 = 0.50;

/// 预览笔迹控制点 —— `_BRUSH_PREVIEW_POINTS`（`gui/drawutils.py:38`）。
///
/// 每行 `[px, py, pressure, xtilt, ytilt]`，坐标是画布尺寸的比例。
/// 它是一条来回划拉的大锯齿，一次展示压感、倾斜与方向动态。
const PREVIEW_POINTS: [[f32; 5]; 14] = [
    [0.00, 0.00, 0.00, 0.00, 0.00],
    [1.00, 0.05, 0.00, -0.06, 0.05],
    [0.10, 0.10, 0.20, 0.10, 0.05],
    [0.90, 0.15, 0.90, -0.05, 0.05],
    [0.11, 0.30, 0.90, 0.08, 0.05],
    [0.86, 0.35, 0.90, -0.04, 0.05],
    [0.13, 0.50, 0.90, 0.06, 0.05],
    [0.84, 0.55, 0.90, -0.03, 0.05],
    [0.17, 0.70, 0.90, 0.04, 0.05],
    [0.83, 0.75, 0.90, -0.02, 0.05],
    [0.25, 0.90, 0.20, 0.02, 0.00],
    [0.81, 0.95, 0.00, 0.00, 0.00],
    [0.41, 0.95, 0.00, 0.00, 0.00],
    [0.80, 1.00, 0.00, 0.00, 0.00],
];

/// 预览事件之间的时间步长（秒）——`_variable_pressure_scribble` 的 `event_dtime`。
const EVENT_DTIME: f64 = 0.005;
/// 走完两个控制点之间的时间（秒）——同函数的 `point_time`。
const POINT_TIME: f64 = 0.1;

/// Catmull-Rom 样条插值 —— `spline_4p`（`gui/drawutils.py:60`）。
///
/// 返回 p0→p1 之间、参数 `t ∈ [0,1]` 处的插值点。
fn spline_4p(t: f32, p_1: &[f32; 5], p0: &[f32; 5], p1: &[f32; 5], p2: &[f32; 5]) -> [f32; 5] {
    let mut out = [0.0f32; 5];
    for i in 0..5 {
        out[i] = (t * ((2.0 - t) * t - 1.0) * p_1[i]
            + (t * t * (3.0 * t - 5.0) + 2.0) * p0[i]
            + t * ((4.0 - 3.0 * t) * t + 1.0) * p1[i]
            + (t - 1.0) * t * t * p2[i])
            / 2.0;
    }
    out
}

/// 控制点序列 → 可插值的 4 元组序列 —— `spline_iter`（`gui/drawutils.py:89`）。
///
/// 首尾各复制一次，使曲线**穿过**首尾点（而不是被它们拉偏）。产出 `n - 1` 段。
fn spline_iter(points: &[[f32; 5]]) -> Vec<[&[f32; 5]; 4]> {
    let n = points.len();
    assert!(n >= 2, "预览笔迹至少需要 2 个控制点");
    let mut out = Vec::with_capacity(n - 1);
    for k in 1..n {
        // 窗口 [p_{k-2}, p_{k-1}, p_k, p_{k+1}]；越界的 p_{-1} 用 p_0 顶替
        // （double_first），p_n 用 p_{n-1} 顶替（double_last）。
        out.push([
            &points[k.saturating_sub(2).min(k - 1)],
            &points[k - 1],
            &points[k],
            &points[if k + 1 < n { k + 1 } else { n - 1 }],
        ]);
    }
    out
}

/// 生成预览笔迹事件流 —— `_variable_pressure_scribble`（`gui/drawutils.py:118`）。
///
/// 产出 `(dtime, x, y, pressure, xtilt, ytilt)`。首尾各有一笔 `dtime = 10`
/// （模拟抬笔停顿，让落笔/收笔状态机跑完）。
fn variable_pressure_scribble(w: f32, h: f32) -> Vec<(f64, f32, f32, f32, f32, f32)> {
    let mut out = Vec::new();
    let first = &PREVIEW_POINTS[0];
    // MyPaint 此处把 press 写死为 0.0；points[0][2] 本来就是 0.0，等价
    out.push((10.0, first[0] * w, first[1] * h, 0.0, first[3], first[4]));

    for [p_1, p0, p1, p2] in spline_iter(&PREVIEW_POINTS) {
        let mut dt = 0.0f64;
        while dt < POINT_TIME {
            let t = (dt / POINT_TIME) as f32;
            let s = spline_4p(t, p_1, p0, p1, p2);
            out.push((EVENT_DTIME, s[0] * w, s[1] * h, s[2], s[3], s[4]));
            dt += EVENT_DTIME;
        }
    }

    let last = &PREVIEW_POINTS[PREVIEW_POINTS.len() - 1];
    out.push((10.0, last[0] * w, last[1] * h, 0.0, last[3], last[4]));
    out
}

/// 某设置是否"恒定"（没有任何输入曲线）—— `has_only_base_value`（`lib/brush.py:545`）。
fn has_only_base_value(brush: &Brush, s: BrushSetting) -> bool {
    brush.get(s).inputs.is_empty()
}

/// 基础值是否大于阈值 —— `has_large_base_value`（`lib/brush.py:552`，默认阈值 0.9）。
fn has_large_base_value(brush: &Brush, s: BrushSetting) -> bool {
    brush.get(s).base_value > 0.9
}

/// 基础值是否小于阈值 —— `has_small_base_value`（`lib/brush.py:555`，默认阈值 0.1）。
fn has_small_base_value(brush: &Brush, s: BrushSetting) -> bool {
    brush.get(s).base_value < 0.1
}

/// 铺预览底色并给出前景色 —— `_brush_preview_bg_fg`（`gui/drawutils.py:203`）。
///
/// 返回 `(前景 RGB, spiral 标记)`。底色按笔刷性格三选一；`spiral` 在 MyPaint 里
/// 算出来但**从未被读取**（`render_brush_preview_pixbuf` 接收后没用到），
/// 这里照样返回以保留可追溯性。
fn brush_preview_bg_fg(
    surface: &mut PigmentSurface,
    size_in_tiles: usize,
    brush: &Brush,
) -> ([f32; 3], bool) {
    // 默认：纸黄灰。col1 / col2 只差一点，构成对角明暗
    let mut col1 = [0.85f32, 0.85, 0.80];
    let mut col2 = [0.80f32, 0.80, 0.80];
    let mut fgcol = [0.05f32, 0.15, 0.20];
    let mut spiral = false;

    // 三档性格：橡皮（粉红）/ colorize（橙）/ smudge（偏蓝）
    const FX: [(BrushSetting, [f32; 3], [f32; 3], bool, [f32; 3]); 3] = [
        (
            BrushSetting::Eraser,
            [0.8, 0.7, 0.7],
            [0.75, 0.60, 0.60],
            false,
            [0.05, 0.15, 0.20],
        ),
        (
            BrushSetting::Colorize,
            [0.8, 0.8, 0.8],
            [0.6, 0.6, 0.6],
            false,
            [0.6, 0.2, 0.0],
        ),
        (
            BrushSetting::Smudge,
            [0.85, 0.85, 0.80],
            [0.60, 0.60, 0.70],
            true,
            [0.05, 0.15, 0.20],
        ),
    ];
    for (setting, c1, c2, c_spiral, c_fg) in FX {
        if has_large_base_value(brush, setting) {
            col1 = c1;
            col2 = c2;
            fgcol = c_fg;
            spiral = c_spiral;
            break;
        }
    }

    // 「从不涂抹」的笔刷两色合一（否则对角线上会出现莫名色块）
    let never_smudger = has_small_base_value(brush, BrushSetting::Smudge)
        && has_only_base_value(brush, BrushSetting::Smudge);
    let colorizer = has_large_base_value(brush, BrushSetting::Colorize);
    if never_smudger && !colorizer {
        col2 = col1;
    }

    // fix15 满值 = 1<<15（libmypaint 的 `a = 1 << 15`）
    let a = 1u16 << 15;
    let to_fix = |c: [f32; 3]| -> [u16; 4] {
        [
            fix15::from_f32(c[0]),
            fix15::from_f32(c[1]),
            fix15::from_f32(c[2]),
            a,
        ]
    };
    let col1_fix = to_fix(col1);
    let col2_fix = to_fix(col2);

    for ty in 0..size_in_tiles {
        let tx_thres = size_in_tiles.saturating_sub(ty + 1);
        for tx in 0..size_in_tiles {
            let (mut topcol, mut botcol) = (col1_fix, col1_fix);
            if tx > tx_thres {
                topcol = col2_fix;
            }
            if tx >= tx_thres {
                botcol = col2_fix;
            }
            let t = surface.tile_request_start(tx as i32, ty as i32);
            if topcol == botcol {
                for y in 0..TILE_SIZE {
                    for x in 0..TILE_SIZE {
                        t[y][x] = topcol;
                    }
                }
            } else {
                // 逐列切对角：第 `x` 列上半（y < N-x）取 topcol，其余取 botcol
                for x in 0..TILE_SIZE {
                    let lim = TILE_SIZE - x;
                    for y in 0..lim {
                        t[y][x] = topcol;
                    }
                    for y in lim..TILE_SIZE {
                        t[y][x] = botcol;
                    }
                }
            }
            surface.tile_request_end(tx as i32, ty as i32);
        }
    }

    (fgcol, spiral)
}

/// 渲染一支笔刷的自动预览 —— `render_brush_preview_pixbuf`（`gui/drawutils.py:135`）。
///
/// 返回 128×128 的 RGBA8（直通 alpha），可直接交给
/// `egui::ColorImage::from_rgba_unmultiplied`。
///
/// `color_*` 用**笔刷自带**的值：MyPaint 只把前景色 `set_color_rgb(fg)` 写进去，
/// 不改笔刷颜色。
pub fn render(brush: &Brush) -> Vec<u8> {
    let n = TILE_SIZE;
    let mut width = n;
    let mut size_used = 1usize;
    let mut surface = PigmentSurface::new();

    // MyPaint: `for size_in_tiles in range(1, max_edge_tiles)` —— 画布不够大就换下一档重画
    for size_in_tiles in 1..MAX_EDGE_TILES {
        width = n * size_in_tiles;
        surface = PigmentSurface::new();
        let (fg, _spiral) = brush_preview_bg_fg(&mut surface, size_in_tiles, brush);

        let mut b = brush.clone();
        // set_color_rgb：RGB → HSV 后写进 color_h/s/v 的 base_value
        let hsv = rgb_to_hsv(fg);
        b.get_mut(BrushSetting::ColorH).base_value = hsv[0];
        b.get_mut(BrushSetting::ColorS).base_value = hsv[1];
        b.get_mut(BrushSetting::ColorV).base_value = hsv[2];

        let mut state = BrushState::default();
        state.reset();

        let shape = variable_pressure_scribble(width as f32, width as f32);
        surface.begin_atomic();
        for (dt, x, y, p, xt, yt) in shape {
            b.stroke_to_full(
                &mut state,
                &mut surface,
                x,
                y,
                p,
                xt,
                yt,
                dt,
                1.0,
                0.0,
                0.0,
                true,
            );
        }
        surface.end_atomic();

        size_used = size_in_tiles;
        // 笔画是否溢出画布？MyPaint 用 tiledict 的键范围判定（drawutils.py:172）
        let outside = surface.tiles().any(|((tx, ty), _)| {
            tx < 0 || ty < 0 || tx >= size_in_tiles as i32 || ty >= size_in_tiles as i32
        });
        if !outside {
            break;
        }
    }

    // 把 0..size 的 tile 拼成一张 width×width 的 RGBA8
    let mut rgba = vec![0u8; width * width * 4];
    for ty in 0..size_used {
        for tx in 0..size_used {
            let Some(pixels) = surface.tile(tx as i32, ty as i32) else {
                continue;
            };
            let conv = tile_to_rgba8(pixels);
            for y in 0..TILE_SIZE {
                for x in 0..TILE_SIZE {
                    let px = tx * TILE_SIZE + x;
                    let py = ty * TILE_SIZE + y;
                    let di = (py * width + px) * 4;
                    rgba[di..di + 4].copy_from_slice(&conv[y * TILE_SIZE + x]);
                }
            }
        }
    }
    let pixbuf = image::RgbaImage::from_raw(width as u32, width as u32, rgba)
        .expect("预览画布尺寸与缓冲不符");

    // 缩放到 128×128：比 128 小用 NEAREST（保像素）、比 128 大用 BILINEAR
    let scaled = if width != PREVIEW_W {
        let filter = if width < PREVIEW_W {
            image::imageops::FilterType::Nearest
        } else {
            // GdkPixbuf 的 BILINEAR ≈ image 的 Triangle
            image::imageops::FilterType::Triangle
        };
        image::imageops::resize(&pixbuf, PREVIEW_W as u32, PREVIEW_H as u32, filter)
    } else {
        pixbuf
    };

    // 叠在棋盘格上（显示"擦除"效果）
    composite_checks(scaled.as_raw())
}

/// 一块 tile → 64 个 RGBA8 —— `mypaintlib.tile_convert_rgba16_to_rgba8`。
///
/// 算式照抄 `lib/pixops.cpp:227`（伽马分支）：
/// 去预乘（带舍入）→ `pow(c, 1/EOTF) * 255 + 0.5`。
///
/// 技术栈适配：省掉了 `dithering_noise`（幅度约 ±2% 的固定伪随机抖动）。
fn tile_to_rgba8(pixels: &TilePixels) -> Vec<[u8; 4]> {
    let mut out = Vec::with_capacity(TILE_SIZE * TILE_SIZE);
    for y in 0..TILE_SIZE {
        for x in 0..TILE_SIZE {
            let p = pixels[y][x];
            let a = p[3] as u32;
            let (r, g, b) = if a != 0 {
                let rnd = a / 2;
                (
                    (((p[0] as u32) << 15) + rnd) / a,
                    (((p[1] as u32) << 15) + rnd) / a,
                    (((p[2] as u32) << 15) + rnd) / a,
                )
            } else {
                (0, 0, 0)
            };
            let ch = |v: u32| -> u8 {
                (crate::convert::eotf_encode(v as f32 / (1u32 << 15) as f32) * 255.0 + 0.5) as u8
            };
            out.push([
                ch(r),
                ch(g),
                ch(b),
                ((a * 255 + (1u32 << 14)) / (1u32 << 15)) as u8,
            ]);
        }
    }
    out
}

/// 把 128×128 的 RGBA 图叠到 16px 灰棋盘上 —— `render_checks` + `cr.paint`。
fn composite_checks(src: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; PREVIEW_W * PREVIEW_H * 4];
    for y in 0..PREVIEW_H {
        for x in 0..PREVIEW_W {
            let i = (y * PREVIEW_W + x) * 4;
            // 棋盘基色 = COLOR_1，`(i + j) % 2 != 0` 处换 COLOR_2
            let base = if (x / ALPHA_CHECK_SIZE + y / ALPHA_CHECK_SIZE) % 2 != 0 {
                ALPHA_CHECK_COLOR_2
            } else {
                ALPHA_CHECK_COLOR_1
            };
            let a = src[i + 3] as f32 / 255.0;
            for c in 0..3 {
                let s = src[i + c] as f32 / 255.0;
                out[i + c] = ((s * a + base * (1.0 - a)) * 255.0 + 0.5) as u8;
            }
            out[i + 3] = 255;
        }
    }
    out
}

/// RGB → HSV —— 照抄 `lib/helpers.py:rgb_to_hsv`（即 `colorsys.rgb_to_hsv`）。
fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0].clamp(0.0, 1.0);
    let g = rgb[1].clamp(0.0, 1.0);
    let b = rgb[2].clamp(0.0, 1.0);
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let v = maxc;
    if minc == maxc {
        return [0.0, 0.0, v];
    }
    let s = (maxc - minc) / maxc;
    let rc = (maxc - r) / (maxc - minc);
    let gc = (maxc - g) / (maxc - minc);
    let bc = (maxc - b) / (maxc - minc);
    let h = (if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    }) / 6.0;
    let h = h % 1.0;
    [if h < 0.0 { h + 1.0 } else { h }, s, v]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::hokusai::BrushInput;

    /// 取内嵌官方包里的某支笔刷。
    fn official(key: &str) -> Brush {
        let lib = pigment_engine::brushlib::BrushLibrary::embedded();
        let i = lib.find(key).unwrap_or_else(|| panic!("缺少 {key}"));
        lib.entries[i].load().expect("解析失败")
    }

    #[test]
    fn spline_iter_yields_n_minus_one_segments() {
        assert_eq!(spline_iter(&PREVIEW_POINTS).len(), PREVIEW_POINTS.len() - 1);
    }

    #[test]
    fn spline_hits_endpoints() {
        // t=0 应回到 p0，t=1 应回到 p1（Catmull-Rom 穿过控制点）
        let pts = PREVIEW_POINTS;
        let seg = spline_iter(&pts);
        let [p_1, p0, p1, p2] = seg[0];
        let a = spline_4p(0.0, p_1, p0, p1, p2);
        for i in 0..5 {
            assert!((a[i] - p0[i]).abs() < 1e-5, "t=0 未落在 p0 上（分量 {i}）");
        }
        let b = spline_4p(1.0, p_1, p0, p1, p2);
        for i in 0..5 {
            assert!((b[i] - p1[i]).abs() < 1e-5, "t=1 未落在 p1 上（分量 {i}）");
        }
    }

    /// 事件流长度 = 首 + (段数 × 每段事件数) + 尾，且每段固定 20 个事件
    /// （`point_time / event_dtime = 0.1 / 0.005 = 20`）。
    #[test]
    fn scribble_event_count_matches_mypaint() {
        let ev = variable_pressure_scribble(64.0, 64.0);
        let per_segment = (POINT_TIME / EVENT_DTIME) as usize;
        assert_eq!(ev.len(), 2 + (PREVIEW_POINTS.len() - 1) * per_segment);
        assert_eq!(ev[0].0, 10.0);
        assert_eq!(ev[ev.len() - 1].0, 10.0);
    }

    /// 橡皮笔刷的底色是粉红（`eraser` 分支），不是默认纸黄灰。
    #[test]
    fn eraser_gets_pink_background() {
        let mut s = PigmentSurface::new();
        let mut b = Brush::new();
        b.get_mut(BrushSetting::Eraser).base_value = 1.0;
        let (fg, _) = brush_preview_bg_fg(&mut s, 1, &b);
        assert_eq!(fg, [0.05, 0.15, 0.20], "橡皮的前景色沿用默认值");
        // 底色应从 tile 读回：左上角（y=0,x=0）必是 col1 = (0.8,0.7,0.7)
        let t = s.tile(0, 0).expect("底色应写入 tile");
        let px = t[0][0];
        assert!((fix15::to_f32(px[0]) - 0.8).abs() < 1e-3);
        assert!((fix15::to_f32(px[1]) - 0.7).abs() < 1e-3);
    }

    /// smudge 笔刷走的是"偏蓝"分支，且带 spiral 标记。
    #[test]
    fn smudge_uses_blue_background_and_spiral_flag() {
        let mut s = PigmentSurface::new();
        let mut b = Brush::new();
        b.get_mut(BrushSetting::Smudge).base_value = 1.0;
        let (_fg, spiral) = brush_preview_bg_fg(&mut s, 1, &b);
        assert!(spiral, "smudge 分支的 spiral 应为 true");
        let t = s.tile(0, 0).expect("底色应写入 tile");
        let px = t[0][0];
        // col1 = (0.85, 0.85, 0.80)：蓝分量最低
        assert!((fix15::to_f32(px[2]) - 0.80).abs() < 1e-3);
        assert!(fix15::to_f32(px[0]) > fix15::to_f32(px[2]));
    }

    /// 「从不涂抹」的笔刷对角双色合一（col2 := col1）→ 整块底色一致。
    #[test]
    fn never_smudger_has_uniform_background() {
        let mut s = PigmentSurface::new();
        let mut b = Brush::new();
        b.get_mut(BrushSetting::Smudge).base_value = 0.0; // < 0.1 且无输入曲线
        brush_preview_bg_fg(&mut s, 2, &b);
        let t00 = s.tile(0, 0).unwrap()[0][0];
        let t11 = s.tile(1, 1).unwrap()[TILE_SIZE - 1][TILE_SIZE - 1];
        assert_eq!(t00, t11, "col2 应被 col1 顶替，底色处处相同");
    }

    /// 有输入曲线的 smudge 不算「从不涂抹」→ 对角双色保留。
    ///
    /// 对角法则（`_brush_preview_bg_fg`）：`x + y < N` 取 topcol，其余取 botcol。
    /// size_in_tiles = 1 时 tile 级判定给出 topcol = col1、botcol = col2，
    /// 所以左下（x+y 小）是 col1、右上（x+y 大）是 col2。
    #[test]
    fn smudge_with_curve_keeps_two_tone_background() {
        let mut s = PigmentSurface::new();
        let mut b = Brush::new();
        b.get_mut(BrushSetting::Smudge).base_value = 0.0;
        b.get_mut(BrushSetting::Smudge).inputs = vec![pigment_engine::hokusai::InputMapping {
            input: BrushInput::Pressure,
            points: vec![(0.0, 0.0), (1.0, 0.5)],
        }];
        brush_preview_bg_fg(&mut s, 1, &b);
        let t = s.tile(0, 0).unwrap();
        let col1 = t[0][0]; // x+y = 0
        let col2 = t[TILE_SIZE - 1][TILE_SIZE - 1]; // x+y = 2N-2
        assert_ne!(col1, col2, "对角双色应保留");
        // col1 = 纸黄灰 (0.85,0.85,0.80)，col2 = 默认第二色 (0.80,0.80,0.80)
        assert!(fix15::to_f32(col1[0]) > fix15::to_f32(col2[0]));
    }

    #[test]
    fn render_returns_128x128_opaque() {
        let b = official("Dieterle/Fan#1");
        let px = render(&b);
        assert_eq!(px.len(), PREVIEW_W * PREVIEW_H * 4);
        // 叠过棋盘格 → 全不透明
        assert!(px.iter().skip(3).step_by(4).all(|&a| a == 255));
    }

    /// 非橡皮笔刷的底色不透明 → 棋盘格被完全盖住，右下角就是纸黄灰 col1。
    #[test]
    fn opaque_background_covers_the_checkerboard() {
        let b = official("classic/dry_brush");
        let px = render(&b);
        let i = ((PREVIEW_H - 8) * PREVIEW_W + (PREVIEW_W - 8)) * 4;
        let expect = (crate::convert::eotf_encode(0.85) * 255.0).round() as u8;
        assert_eq!(px[i], expect, "右下角应为 col1 纸黄灰");
        assert_eq!(px[i + 1], expect);
    }

    /// `composite_checks` 本身：全透明输入时输出必须严格等于棋盘格。
    #[test]
    fn composite_checks_reproduces_the_alpha_checkerboard() {
        let src = vec![0u8; PREVIEW_W * PREVIEW_H * 4];
        let out = composite_checks(&src);
        let c1 = (ALPHA_CHECK_COLOR_1 * 255.0 + 0.5) as u8;
        let c2 = (ALPHA_CHECK_COLOR_2 * 255.0 + 0.5) as u8;
        assert_eq!(out[0], c1, "(0,0) 应为 COLOR_1");
        // `render_checks` 在 `(i + j) % 2 != 0` 处画 COLOR_2 → (0,1) 格是 COLOR_2
        let idx = (ALPHA_CHECK_SIZE * PREVIEW_W) * 4; // (x=0, y=16)
        assert_eq!(out[idx], c2, "(0,16) 应为 COLOR_2");
        // (16,16) 的 (i+j) 为偶数 → 回到 COLOR_1
        let idx2 = (ALPHA_CHECK_SIZE * PREVIEW_W + ALPHA_CHECK_SIZE) * 4;
        assert_eq!(out[idx2], c1, "(16,16) 应为 COLOR_1");
        assert!(out.iter().skip(3).step_by(4).all(|&a| a == 255));
    }

    /// 橡皮笔刷会真的"擦掉"预览底色 → 棋盘格透过擦除区可见
    /// （这正是 `drawutils.py:191` 叠棋盘格的目的）。
    ///
    /// 注意擦除是**部分**的：软橡皮留下残余 alpha，所以透过来的不是棋盘原色
    /// 而是它被残余色调制后的中性灰。判据取「中性灰且亮度落在棋盘区间内」，
    /// 因为预览里的另外两种颜色都不是中性灰（纸黄灰底偏黄、橡皮前景偏蓝）。
    #[test]
    fn eraser_preview_lets_the_checkerboard_through() {
        let b = official("deevad/kneaded_eraser_large");
        let px = render(&b);
        let neutral = px
            .chunks(4)
            .filter(|c| {
                let (r, g, bl) = (c[0] as i32, c[1] as i32, c[2] as i32);
                (r - g).abs() <= 3 && (g - bl).abs() <= 3 && (100..=160).contains(&r)
            })
            .count();
        assert!(
            neutral > 50,
            "橡皮预览应能从擦除区看见棋盘格，中性灰像素只有 {neutral}"
        );
    }

    /// EOTF 应为 2.2（MyPaint 2.x 默认），不是 1.0 的 1.x 兼容模式。
    ///
    /// 画布显示（[`crate::convert`]）与预览图共用同一常量，这里一并钉住。
    #[test]
    fn eotf_default_matches_mypaint() {
        assert_eq!(pigment_surface::color::EOTF, 2.2);
    }
}
