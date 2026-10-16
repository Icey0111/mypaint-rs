//! 行为对等测试（P1 验收）：pigment-surface 与 hokusai 参考后端 MemSurface
//! 在相同笔画事件流下必须**逐像素一致**；光谱冒烟断言在自家后端上复跑。
//!
//! 覆盖矩阵：硬笔刷（含随机偏移 PRNG）、软抗锯齿椭圆 dab、光谱 paint_mode、
//! smudge 反馈回路、橡皮擦、colorize、posterize+lock_alpha、官方 .myb 笔刷。
//!
//! 运行（在 pigment workspace 根目录）：
//!   cargo test -p pigment-surface

use std::path::Path;

use hokusai::fix15;
use hokusai::tile_mem::MemSurface;
use hokusai::{Brush, BrushSetting, BrushState, TiledSurface};
use pigment_surface::PigmentSurface;

fn set(b: &mut Brush, s: BrushSetting, v: f32) {
    b.get_mut(s).base_value = v;
}

/// 通用基础：确定性 dab 节奏（关掉按时间出 dab），半径约 12px。
fn base_brush() -> Brush {
    let mut b = Brush::new();
    set(&mut b, BrushSetting::Radius, 2.48); // ln(12)
    set(&mut b, BrushSetting::DabsPerBasicRadius, 2.0);
    set(&mut b, BrushSetting::DabsPerActualRadius, 2.0);
    set(&mut b, BrushSetting::DabsPerSecond, 0.0);
    set(&mut b, BrushSetting::ColorH, 0.08); // 暖橙
    set(&mut b, BrushSetting::ColorS, 0.85);
    set(&mut b, BrushSetting::ColorV, 0.9);
    b
}

/// ① 硬笔刷 + 随机偏移（顺带验证逐笔画 PRNG 在两个后端间确定性一致）
fn hard_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Hardness, 0.9);
    set(&mut b, BrushSetting::Opaque, 0.9);
    set(&mut b, BrushSetting::OffsetByRandom, 0.9);
    b
}

/// ② 软笔刷 + 抗锯齿 + 椭圆 dab
fn soft_ellipse_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Hardness, 0.15);
    set(&mut b, BrushSetting::AntiAliasing, 1.0);
    set(&mut b, BrushSetting::EllipticalDabRatio, 1.8);
    set(&mut b, BrushSetting::EllipticalDabAngle, 30.0);
    set(&mut b, BrushSetting::Opaque, 0.6);
    b
}

/// ③ 光谱笔刷（paint_mode=1）+ 轻 smudge —— M4 核心路径
fn spectral_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Paint, 1.0);
    set(&mut b, BrushSetting::Smudge, 0.4);
    set(&mut b, BrushSetting::SmudgeLength, 0.5);
    set(&mut b, BrushSetting::Opaque, 0.7);
    set(&mut b, BrushSetting::ColorH, 0.647); // 群青
    set(&mut b, BrushSetting::ColorS, 0.73);
    set(&mut b, BrushSetting::ColorV, 0.75);
    b
}

/// ④ 重 smudge 笔刷 —— 反馈回路压力测试
fn smudge_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Smudge, 0.85);
    set(&mut b, BrushSetting::SmudgeLength, 0.3);
    set(&mut b, BrushSetting::Opaque, 0.4);
    b
}

/// ⑤ 橡皮擦
fn eraser_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Eraser, 1.0);
    set(&mut b, BrushSetting::Opaque, 0.8);
    b
}

/// ⑥ colorize
fn colorize_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Colorize, 1.0);
    set(&mut b, BrushSetting::ColorH, 0.55);
    set(&mut b, BrushSetting::ColorS, 0.5);
    set(&mut b, BrushSetting::ColorV, 0.6);
    b
}

/// ⑦ posterize + lock_alpha 叠加
fn posterize_lock_brush() -> Brush {
    let mut b = base_brush();
    set(&mut b, BrushSetting::Posterize, 0.9);
    set(&mut b, BrushSetting::PosterizeNum, 4.0);
    set(&mut b, BrushSetting::LockAlpha, 0.7);
    set(&mut b, BrushSetting::Opaque, 0.7);
    b
}

/// 一条压力变化的正弦曲线笔画脚本：(x, y, pressure)
fn sine_script(x0: f32, y0: f32, len: f32, amp: f32, steps: usize) -> Vec<(f32, f32, f32)> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let x = x0 + t * len;
            let y = y0 + (t * std::f32::consts::TAU).sin() * amp;
            let p = 0.2 + 0.8 * (t * std::f32::consts::PI).sin(); // 两端轻中间重
            (x, y, p)
        })
        .collect()
}

fn drive<S: TiledSurface>(surface: &mut S, brush: &Brush, script: &[(f32, f32, f32)]) {
    let mut state = BrushState::default();
    surface.begin_atomic();
    for &(x, y, p) in script {
        brush.stroke_to(&mut state, surface, x, y, p, 0.0, 0.0, 0.01);
    }
    surface.end_atomic();
}

/// 在 ±range 的 tile 范围内逐 tile 逐像素对比两个后端。
fn assert_surfaces_identical(a: &PigmentSurface, b: &MemSurface, range: i32) {
    let mut compared = 0usize;
    for ty in -range..=range {
        for tx in -range..=range {
            let (pa, pb) = (a.tile(tx, ty), b.tile(tx, ty));
            match (pa, pb) {
                (None, None) => {}
                (Some(_), None) => panic!("tile ({tx},{ty}) 仅存在于 pigment-surface"),
                (None, Some(_)) => panic!("tile ({tx},{ty}) 仅存在于 MemSurface"),
                (Some(pa), Some(pb)) => {
                    compared += 1;
                    assert_eq!(pa, pb, "tile ({tx},{ty}) 像素不一致");
                }
            }
        }
    }
    assert!(compared > 0, "没有采到任何已分配 tile，测试无效");
    println!("  逐像素对比通过：{compared} 个 tile 完全一致");
}

/// 七组程序化笔刷依次画在同一对画布上，验证全链路对等。
#[test]
fn scripted_strokes_pixel_identical_to_memsurface() {
    let mut ours = PigmentSurface::new();
    let mut reference = MemSurface::new();

    let scenarios: Vec<(Brush, Vec<(f32, f32, f32)>)> = vec![
        (hard_brush(), sine_script(20.0, 60.0, 200.0, 25.0, 60)),
        (soft_ellipse_brush(), sine_script(30.0, 130.0, 180.0, 20.0, 50)),
        (spectral_brush(), sine_script(40.0, 40.0, 160.0, 60.0, 50)),
        (smudge_brush(), sine_script(20.0, 100.0, 220.0, 40.0, 70)),
        (eraser_brush(), sine_script(60.0, 80.0, 120.0, 30.0, 30)),
        (colorize_brush(), sine_script(80.0, 150.0, 100.0, 15.0, 25)),
        (posterize_lock_brush(), sine_script(50.0, 180.0, 140.0, 20.0, 35)),
    ];

    for (i, (brush, script)) in scenarios.iter().enumerate() {
        drive(&mut ours, brush, script);
        drive(&mut reference, brush, script);
        println!("场景 {} 绘制完成", i + 1);
    }

    assert_surfaces_identical(&ours, &reference, 6);
    println!(
        "  pigment-surface: {} tiles / {} KiB",
        ours.tile_count(),
        ours.memory_bytes() / 1024
    );
}

/// 官方 .myb 笔刷（paint_mode 光谱笔刷）在两个后端上的对等验证。
#[test]
fn official_myb_brushes_pixel_identical() {
    let brushes_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../mypaint-brushes/brushes");
    let picks = [
        "Dieterle/Blender.myb",      // 光谱混色 + 重 smudge
        "Dieterle/8B_Pencil#1.myb",  // paint_mode 铅笔
    ];

    for rel in picks {
        let path = brushes_dir.join(rel);
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读取 {rel} 失败: {e}"));
        let brush = hokusai::myb::from_str(&json)
            .unwrap_or_else(|e| panic!("解析 {rel} 失败: {e:?}"));

        let mut ours = PigmentSurface::new();
        let mut reference = MemSurface::new();
        // 先铺一层底色（smudge 笔刷需要有东西可抹），再画目标笔刷
        let undercoat = hard_brush();
        let script_a = sine_script(30.0, 80.0, 180.0, 30.0, 50);
        let script_b = sine_script(30.0, 120.0, 180.0, 40.0, 60);
        drive(&mut ours, &undercoat, &script_a);
        drive(&mut ours, &brush, &script_b);
        drive(&mut reference, &undercoat, &script_a);
        drive(&mut reference, &brush, &script_b);
        println!("{rel}:");
        assert_surfaces_identical(&ours, &reference, 6);
    }
}

/// 光谱冒烟断言在自家后端复跑（P0 的 M4 最小验证，P1 门禁）：
/// 镉黄 + 群青 50/50 对称单 dab → 光谱组绿色主导且更深，RGB 对照组发灰。
#[test]
fn spectral_smoke_on_pigment_surface() {
    fn make_brush(h: f32, s: f32, v: f32, paint_mode: f32) -> Brush {
        let mut b = Brush::new();
        set(&mut b, BrushSetting::Opaque, 0.5);
        set(&mut b, BrushSetting::Radius, 3.69); // ~40px
        set(&mut b, BrushSetting::Hardness, 0.8);
        set(&mut b, BrushSetting::DabsPerBasicRadius, 8.0);
        set(&mut b, BrushSetting::DabsPerActualRadius, 8.0);
        set(&mut b, BrushSetting::DabsPerSecond, 0.0);
        set(&mut b, BrushSetting::OffsetByRandom, 0.0);
        set(&mut b, BrushSetting::Smudge, 0.0);
        set(&mut b, BrushSetting::Paint, paint_mode);
        set(&mut b, BrushSetting::ColorH, h);
        set(&mut b, BrushSetting::ColorS, s);
        set(&mut b, BrushSetting::ColorV, v);
        b
    }

    fn single_dab(brush: &Brush, surface: &mut PigmentSurface, x: f32, y: f32) {
        let mut state = BrushState::default();
        brush.stroke_to(&mut state, surface, x - 3.0, y, 1.0, 0.0, 0.0, 0.01);
        brush.stroke_to(&mut state, surface, x, y, 1.0, 0.0, 0.0, 0.01);
    }

    fn overlap_mean(surface: &PigmentSurface) -> (f32, f32, f32) {
        // 两个 dab 圆心 (165,150) / (235,150)，透镜重叠区中心约 (200,150)
        let ts = 64i32;
        let (mut sr, mut sg, mut sb, mut n) = (0.0f32, 0.0f32, 0.0f32, 0u32);
        for wy in 147i32..=153 {
            for wx in 197i32..=203 {
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
        assert!(n > 0, "重叠区没有采到像素");
        (sr / n as f32, sg / n as f32, sb / n as f32)
    }

    let run = |paint_mode: f32| {
        let mut surface = PigmentSurface::new();
        single_dab(&make_brush(0.144, 0.90, 1.00, paint_mode), &mut surface, 165.0, 150.0);
        single_dab(&make_brush(0.647, 0.73, 0.75, paint_mode), &mut surface, 235.0, 150.0);
        overlap_mean(&surface)
    };

    let pigment = run(1.0);
    let control = run(0.0);
    println!("光谱组 linear={pigment:?}  对照组 linear={control:?}");

    assert!(
        pigment.1 > pigment.0 && pigment.1 > pigment.2,
        "光谱混合未呈现绿色主导: {pigment:?}"
    );
    assert!(
        pigment.0 + pigment.1 + pigment.2 < control.0 + control.1 + control.2,
        "光谱组不比对照组深: pigment={pigment:?} control={control:?}"
    );
    let spread = |t: (f32, f32, f32)| t.0.max(t.1).max(t.2) - t.0.min(t.1).min(t.2);
    assert!(spread(control) < 0.08, "对照组不够灰: {control:?}");
}
