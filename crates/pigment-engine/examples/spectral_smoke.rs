//! 光谱混色冒烟测试：黄 + 蓝 = 绿（颜料物理），而非 RGB 插值的灰。
//!
//! 一条黄色横笔画 + 一条蓝色竖笔画交叉；paint_mode=1 走 10 波段 WGM 光谱混合，
//! paint_mode=0 作为对照组走普通 alpha 混合。采样交叉点验证绿色主导。
//!
//! 运行（在 pigment workspace 根目录）：
//!   cargo run -p pigment-engine --example spectral_smoke
//! 产出：spectral_smoke_pigment.png / spectral_smoke_rgb.png

use hokusai::color::linear_to_srgb;
use hokusai::tile::TILE_SIZE;
use hokusai::tile_mem::MemSurface;
use hokusai::{fix15, Brush, BrushSetting, BrushState};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
/// 交叉点坐标
const CX: i32 = 200;
const CY: i32 = 150;

fn make_brush(h: f32, s: f32, v: f32, paint_mode: f32) -> Brush {
    let mut b = Brush::new();
    let mut set = |s_: BrushSetting, v_: f32| b.get_mut(s_).base_value = v_;
    set(BrushSetting::Opaque, 0.5); // 单个 dab 半透：两个 dab 恰好 50/50 对称混合
    set(BrushSetting::Radius, 3.69); // ln(40) ≈ 3.69 → 半径约 40px
    set(BrushSetting::Hardness, 0.8);
    // dab 密度调高，保证 3px 的短笔画也能恰好放出 1 个 dab
    set(BrushSetting::DabsPerBasicRadius, 8.0);
    set(BrushSetting::DabsPerActualRadius, 8.0);
    set(BrushSetting::DabsPerSecond, 0.0);
    set(BrushSetting::OffsetByRandom, 0.0); // 关掉随机偏移，保证确定性
    set(BrushSetting::Smudge, 0.0); // 纯混合模式验证，不走 smudge 反馈
    set(BrushSetting::Paint, paint_mode);
    set(BrushSetting::ColorH, h);
    set(BrushSetting::ColorS, s);
    set(BrushSetting::ColorV, v);
    b
}

/// 在 (x, y) 处画恰好约 1 个 dab（首次调用只播种位置，短移动放 1 个 dab）
fn single_dab(brush: &Brush, surface: &mut MemSurface, x: f32, y: f32) {
    let mut state = BrushState::default();
    brush.stroke_to(&mut state, surface, x - 3.0, y, 1.0, 0.0, 0.0, 0.01);
    brush.stroke_to(&mut state, surface, x, y, 1.0, 0.0, 0.0, 0.01);
}

/// 采样交叉点 ±3px 区域内 alpha > 0.5 的像素，返回去预乘后的线性 RGB 均值
fn sample_overlap(surface: &MemSurface) -> (f32, f32, f32) {
    let ts = TILE_SIZE as i32;
    let (mut sr, mut sg, mut sb, mut n) = (0.0f32, 0.0f32, 0.0f32, 0u32);
    for wy in (CY - 3)..=(CY + 3) {
        for wx in (CX - 3)..=(CX + 3) {
            let Some(tile) = surface.tile(wx.div_euclid(ts), wy.div_euclid(ts)) else {
                continue;
            };
            let p = tile[wy.rem_euclid(ts) as usize][wx.rem_euclid(ts) as usize];
            let a = fix15::to_f32(p[3]);
            if a > 0.5 {
                sr += fix15::to_f32(p[0]) / a;
                sg += fix15::to_f32(p[1]) / a;
                sb += fix15::to_f32(p[2]) / a;
                n += 1;
            }
        }
    }
    assert!(n > 0, "交叉点没有采到任何像素");
    (sr / n as f32, sg / n as f32, sb / n as f32)
}

fn flatten(surface: &MemSurface, path: &str) {
    let mut img = image::RgbaImage::from_pixel(WIDTH, HEIGHT, image::Rgba([255, 255, 255, 255]));
    let ts = TILE_SIZE as i32;
    for ty in 0..(HEIGHT as i32 + ts - 1) / ts {
        for tx in 0..(WIDTH as i32 + ts - 1) / ts {
            let Some(tile) = surface.tile(tx, ty) else { continue };
            for ly in 0..TILE_SIZE {
                for lx in 0..TILE_SIZE {
                    let (wx, wy) = (tx * ts + lx as i32, ty * ts + ly as i32);
                    if wx >= WIDTH as i32 || wy >= HEIGHT as i32 {
                        continue;
                    }
                    let p = tile[ly][lx];
                    let a = fix15::to_f32(p[3]);
                    if a <= 0.0 {
                        continue;
                    }
                    let (r, g, b) = (
                        fix15::to_f32(p[0]) / a,
                        fix15::to_f32(p[1]) / a,
                        fix15::to_f32(p[2]) / a,
                    );
                    img.put_pixel(
                        wx as u32,
                        wy as u32,
                        image::Rgba([
                            (linear_to_srgb((r * a + 1.0 - a).clamp(0.0, 1.0)) * 255.0).round() as u8,
                            (linear_to_srgb((g * a + 1.0 - a).clamp(0.0, 1.0)) * 255.0).round() as u8,
                            (linear_to_srgb((b * a + 1.0 - a).clamp(0.0, 1.0)) * 255.0).round() as u8,
                            255,
                        ]),
                    );
                }
            }
        }
    }
    img.save(path).unwrap();
}

fn run(paint_mode: f32) -> (f32, f32, f32) {
    let mut surface = MemSurface::new();
    // 用真实颜料色而非光谱极限色：镉黄 ≈ (1.0,0.85,0.1)，群青 ≈ (0.2,0.25,0.75)
    // 纯 RGB 蓝的绿色反射率为 0，WGM 里会压死绿色通道，混不出绿
    let yellow = make_brush(0.144, 0.90, 1.00, paint_mode);
    let blue = make_brush(0.647, 0.73, 0.75, paint_mode);
    // 两个 dab 部分重叠成维恩图：黄盘在左、蓝盘在右，中间透镜区即混合区
    single_dab(&yellow, &mut surface, 165.0, CY as f32);
    single_dab(&blue, &mut surface, 235.0, CY as f32);
    let rgb = sample_overlap(&surface);
    let name = if paint_mode > 0.5 { "pigment" } else { "rgb" };
    flatten(&surface, &format!("spectral_smoke_{name}.png"));
    rgb
}

fn main() {
    let pigment = run(1.0);
    let control = run(0.0);

    let fmt = |(r, g, b): (f32, f32, f32)| {
        format!(
            "linear=({r:.3}, {g:.3}, {b:.3})  sRGB≈({:.0}, {:.0}, {:.0})",
            linear_to_srgb(r.clamp(0.0, 1.0)) * 255.0,
            linear_to_srgb(g.clamp(0.0, 1.0)) * 255.0,
            linear_to_srgb(b.clamp(0.0, 1.0)) * 255.0,
        )
    };
    println!("paint_mode=1 (光谱 WGM): {}", fmt(pigment));
    println!("paint_mode=0 (RGB 对照): {}", fmt(control));

    // ① 光谱混合：绿色通道主导（黄蓝颜料混出绿）
    assert!(
        pigment.1 > pigment.0 && pigment.1 > pigment.2,
        "光谱混合未呈现绿色主导: {pigment:?}"
    );
    // ② 颜料逻辑：光谱组应比 RGB 对照组更深更浊（越涂越深的减色混合）
    assert!(
        pigment.0 + pigment.1 + pigment.2 < control.0 + control.1 + control.2,
        "光谱组不比对照组深: pigment={pigment:?} control={control:?}"
    );
    // ③ 对照组：RGB 插值应接近灰（通道差异小）
    let spread = |t: (f32, f32, f32)| {
        let mx = t.0.max(t.1).max(t.2);
        let mn = t.0.min(t.1).min(t.2);
        mx - mn
    };
    assert!(
        spread(control) < 0.08,
        "对照组不够灰: control={control:?}"
    );
    println!("OK: 黄 + 蓝 → 绿（光谱）且更深；对照组为灰（RGB 插值）");
}
