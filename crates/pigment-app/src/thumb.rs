//! 笔刷笔画预览渲染 —— 笔刷列表缩略图与编辑器实时预览共用同一条路径。
//!
//! 三个取舍：
//!
//! 1. **半径归一化**：预览把半径在**对数域整体平移**到 [`PREVIEW_RADIUS`]，
//!    保留压感 / 速度的动态形状，只改绝对大小。否则 200px 的大笔刷在 64px
//!    缩略图里只剩一块糊色，看不出笔尖性格。
//! 2. **浅灰渐变底色**：橡皮 / smudge / blur 这类笔刷本身不产色，空画布上
//!    涂了也看不出效果。铺一层不透明渐变底再把笔画盖上去，这类笔刷立刻可见。
//! 3. **画布恰好一个 tile**：预览窗口取 64×24 世界像素，整体落在 tile(0,0)
//!    内，输出时直接读一块 tile，没有跨 tile 拼接。

use pigment_engine::hokusai::color::linear_to_srgb;
use pigment_engine::hokusai::fix15;
use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushInput, BrushSetting, BrushState, TiledSurface};
use pigment_surface::PigmentSurface;

/// 预览画布宽（世界像素）= 一个 tile 宽。
pub const PREVIEW_W: usize = 64;
/// 预览画布高（世界像素）。
pub const PREVIEW_H: usize = 24;
/// 归一化后的笔画半径（压感 1.0 处）。
pub const PREVIEW_RADIUS: f32 = 6.0;

/// 预览宽高比（供 UI 排布）。
pub const ASPECT: f32 = PREVIEW_W as f32 / PREVIEW_H as f32;

/// 把笔刷半径在压感 1.0 处归一化到 `target`（对数域平移，动态形状不变）。
pub fn normalize_radius(brush: &mut Brush, target: f32) {
    let cur = radius_log_reference(brush);
    brush.get_mut(BrushSetting::Radius).base_value += target.ln() - cur;
}

/// 笔刷在参考输入向量下的 `radius_logarithmic` 输出。
///
/// 速度类输入取 0（静止 = 最粗状态），其余取 1.0 —— 与「一笔落下时」的
/// 观感一致。
fn radius_log_reference(brush: &Brush) -> f32 {
    let sv = brush.get(BrushSetting::Radius);
    let mut v = sv.base_value;
    for m in &sv.inputs {
        let x = match m.input {
            BrushInput::Speed1 | BrushInput::Speed2 | BrushInput::Direction => 0.0,
            _ => 1.0,
        };
        v += m.eval(x);
    }
    v
}

/// 渲染一支笔刷的预览笔画，返回直通 alpha 的 sRGB8 像素（`w * h * 4`）。
///
/// `color` 为 `[h, s, v]`，只覆盖 `color_*` 的 base_value，
/// 保留笔刷自带的颜色动态（与 libmypaint 的 `set_base_value(COLOR_*)` 语义一致）。
pub fn render(brush: &Brush, color: [f32; 3], w: usize, h: usize) -> Vec<u8> {
    let mut b = brush.clone();
    normalize_radius(&mut b, PREVIEW_RADIUS);
    b.get_mut(BrushSetting::ColorH).base_value = color[0];
    b.get_mut(BrushSetting::ColorS).base_value = color[1];
    b.get_mut(BrushSetting::ColorV).base_value = color[2];

    let mut surface = PigmentSurface::new();
    // 底色：线性空间浅灰水平渐变（0.88 → 0.55），不透明
    {
        let t = surface.tile_request_start(0, 0);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                let g = 0.88 + (0.55 - 0.88) * (x as f32 / (TILE_SIZE - 1) as f32);
                let v = fix15::from_f32(g);
                t[y][x] = [v, v, v, 1 << 15];
            }
        }
        surface.tile_request_end(0, 0);
    }

    // 一条正弦笔画，压感 0.35 → 1.0（展示不透明度 / 尺寸动态）
    let mut state = BrushState::default();
    let steps = 48;
    let (cx0, cx1) = (10.0_f32, w as f32 - 10.0);
    let y_mid = h as f32 * 0.5;
    let amp = h as f32 * 0.18;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = cx0 + t * (cx1 - cx0);
        let y = y_mid + (t * std::f32::consts::TAU * 0.9).sin() * amp;
        let pressure = 0.35 + 0.65 * t;
        b.stroke_to(&mut state, &mut surface, x, y, pressure, 0.0, 0.0, 0.01);
    }

    // 输出：去预乘 → sRGB8（直通 alpha，由 egui 负责与面板底色合成）
    let mut out = vec![0u8; w * h * 4];
    if let Some(t) = surface.tile(0, 0) {
        for y in 0..h.min(TILE_SIZE) {
            for x in 0..w.min(TILE_SIZE) {
                let p = t[y][x];
                let a = fix15::to_f32(p[3]);
                if a <= 0.0 {
                    continue;
                }
                let i = (y * w + x) * 4;
                let ch = |v: u16| -> u8 {
                    (linear_to_srgb((fix15::to_f32(v) / a).clamp(0.0, 1.0)) * 255.0).round() as u8
                };
                out[i] = ch(p[0]);
                out[i + 1] = ch(p[1]);
                out[i + 2] = ch(p[2]);
                out[i + 3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一支"会画画"的笔刷。
    ///
    /// **别用 `Brush::new()` 当测试笔刷**：它所有设置都是 0，而
    /// `hardness == 0` 在 libmypaint 里意味着无穷软的笔尖 → 每像素 α 恒为 0
    /// → `draw_dab` 连 tile 都不会碰（实测 tiles=0）。libmypaint 的默认值不在
    /// Rust 结构体里，而在 `.myb` 解析时的 `apply_libmypaint_defaults`。
    fn dummy() -> Brush {
        let mut b = Brush::new();
        b.get_mut(BrushSetting::Radius).base_value = 3.0;
        b.get_mut(BrushSetting::Hardness).base_value = 0.6;
        b.get_mut(BrushSetting::AntiAliasing).base_value = 1.0;
        b.get_mut(BrushSetting::Opaque).base_value = 1.0;
        b.get_mut(BrushSetting::OpaqueLinearize).base_value = 0.9;
        b.get_mut(BrushSetting::DabsPerActualRadius).base_value = 2.0;
        b.get_mut(BrushSetting::DabsPerBasicRadius).base_value = 2.0;
        b
    }

    #[test]
    fn normalization_preserves_dynamic_shape() {
        let mut a = dummy();
        a.get_mut(BrushSetting::Radius).inputs = vec![
            pigment_engine::hokusai::InputMapping {
                input: BrushInput::Pressure,
                points: vec![(0.0, -1.5), (1.0, 0.0)],
            },
        ];
        let mut b = a.clone();
        normalize_radius(&mut a, 8.0);
        normalize_radius(&mut b, 2.0);

        // 压感 1.0 处半径分别等于目标；压感 0.0 处两者比值相同（动态形状一致）
        let r1a = radius_log_reference(&a);
        let r1b = radius_log_reference(&b);
        assert!((r1a.exp() - 8.0).abs() < 0.01, "归一化后应为 8px，实际 {}", r1a.exp());
        assert!((r1b.exp() - 2.0).abs() < 0.01);
        let dyn_a = a.get(BrushSetting::Radius).inputs[0].eval(0.0);
        let dyn_b = b.get(BrushSetting::Radius).inputs[0].eval(0.0);
        assert_eq!(dyn_a, dyn_b, "动态形状不该被归一化改动");
    }

    #[test]
    fn renders_something_and_stays_in_bounds() {
        let b = dummy();
        let px = render(&b, [0.0, 0.9, 0.2], PREVIEW_W, PREVIEW_H);
        assert_eq!(px.len(), PREVIEW_W * PREVIEW_H * 4);
        // 底色不透明 → alpha 全满
        assert!(px.iter().skip(3).step_by(4).all(|&a| a == 255));
        // 笔画留下的暗色像素应确实存在（底色最暗为 sRGB(0.55)≈197）
        let dark = px.chunks(4).filter(|c| c[0] < 150).count();
        assert!(dark > 20, "笔画未落到预览上（暗像素 {dark}）");
    }
}
