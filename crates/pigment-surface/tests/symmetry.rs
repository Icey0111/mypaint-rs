//! 对称绘制的像素级测试。
//!
//! 三道门禁，对应任务里的三条硬要求：
//!
//! 1. **对称关闭 = 与原来逐像素一致**（[`symmetry_off_is_bit_identical_to_reference`]）：
//!    同一个笔刷、同一条笔画脚本，分别画在
//!    (a) 从没碰过对称 API 的 [`PigmentSurface`]、
//!    (b) 调过 `set_symmetry_state(active=false, …)` 的 [`PigmentSurface`]、
//!    (c) hokusai 的参考后端 `MemSurface`（"对称之前的那个实现"）上，
//!    三者逐 tile 逐像素必须**全等**。
//! 2. **镜像位置与镜像角度的正确性**：水平/垂直镜像的位置互为镜像、
//!    alpha 也互为镜像；画在轴上的 dab 镜像后落回原地。
//! 3. **旋转对称的份数**：N 条线 → 原始 + N-1 个 dab，落在 N 等分角上。
//!
//! 运行：`cargo test -p pigment-surface --test symmetry`

use hokusai::color::RgbaF32;
use hokusai::fix15;
use hokusai::tile_mem::MemSurface;
use hokusai::{Brush, BrushSetting, BrushState, Dab, TiledSurface};
use pigment_surface::symmetry::SymmetryType;
use pigment_surface::PigmentSurface;

const TS: i32 = hokusai::TILE_SIZE as i32;

// ---- 通用工具 ----

/// 一个几何/颜色都明确的 dab。
///
/// `paint = 0.0` 是 libmypaint 的非光谱默认（`process_op` 走普通混合分支，
/// `mypaint-tiled-surface.c:512-522`）；`alpha_eraser = 1.0` 对应 C 的
/// `op->color_a == 1.0`（普通笔刷的颜色 alpha），否则 hokusai 会走
/// "擦除感知"那条分支（`brushmodes.rs` 的 `blend_normal_and_eraser`）。
/// `opaque = 0.5` 让"同一位置盖两笔"在 alpha 上看得出来。
fn test_dab(x: f32, y: f32, angle: f32, aspect_ratio: f32) -> Dab {
    Dab {
        x,
        y,
        radius: 5.0,
        color: RgbaF32::new(1.0, 0.0, 0.0, 1.0),
        opaque: 0.5,
        hardness: 1.0,
        alpha_eraser: 1.0,
        aspect_ratio,
        angle,
        lock_alpha: 0.0,
        colorize: 0.0,
        posterize: 0.0,
        posterize_num: 0.0,
        paint: 0.0,
        anti_aliasing: 0.0,
    }
}

/// 画一串 dab（外面套一次原子窗口 —— 对称矩阵正是在 `begin_atomic` 时生效的）。
fn paint(surface: &mut PigmentSurface, dabs: &[Dab]) {
    surface.begin_atomic();
    for d in dabs {
        surface.draw_dab(d);
    }
    surface.end_atomic();
}

/// 某个模型坐标上的 alpha（0..1）。
fn alpha_at(surface: &PigmentSurface, x: i32, y: i32) -> f32 {
    let Some(tile) = surface.tile(x.div_euclid(TS), y.div_euclid(TS)) else {
        return 0.0;
    };
    let p = tile[y.rem_euclid(TS) as usize][x.rem_euclid(TS) as usize];
    fix15::to_f32(p[3])
}

/// 有笔迹的像素外接框 `(x, y, w, h)`（alpha > 0.01），空表面返回 `None`。
fn ink_bbox(surface: &PigmentSurface) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for ((tx, ty), tile) in surface.tiles() {
        for iy in 0..TS as usize {
            for ix in 0..TS as usize {
                if fix15::to_f32(tile.pixels()[iy][ix][3]) > 0.01 {
                    let (x, y) = (tx * TS + ix as i32, ty * TS + iy as i32);
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
    }
    if x0 == i32::MAX {
        None
    } else {
        Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }
}

/// 已分配 tile 坐标（排序后比较用；`tiles()` 的顺序来自 HashMap，不稳定）。
fn tile_coords(surface: &PigmentSurface) -> Vec<(i32, i32)> {
    let mut coords: Vec<(i32, i32)> = surface.tiles().map(|(c, _)| c).collect();
    coords.sort_unstable();
    coords
}

/// 逐 tile 逐像素比较两个 [`PigmentSurface`]。
fn assert_pigment_identical(a: &PigmentSurface, b: &PigmentSurface, what: &str) {
    let coords = tile_coords(a);
    assert_eq!(coords, tile_coords(b), "{what}: 已分配 tile 集合不同");
    assert!(!coords.is_empty(), "{what}: 没有采到任何 tile，测试无效");
    for c in coords {
        assert_eq!(
            a.tile(c.0, c.1),
            b.tile(c.0, c.1),
            "{what}: tile {c:?} 像素不一致"
        );
    }
}

/// 逐 tile 逐像素比较 [`PigmentSurface`] 与参考后端 `MemSurface`。
fn assert_matches_reference(a: &PigmentSurface, b: &MemSurface, what: &str) {
    let mut compared = 0usize;
    for ty in -8..=8 {
        for tx in -8..=8 {
            match (a.tile(tx, ty), b.tile(tx, ty)) {
                (None, None) => {}
                (Some(_), None) => panic!("{what}: tile ({tx},{ty}) 仅存在于 pigment-surface"),
                (None, Some(_)) => panic!("{what}: tile ({tx},{ty}) 仅存在于 MemSurface"),
                (Some(pa), Some(pb)) => {
                    compared += 1;
                    assert_eq!(pa, pb, "{what}: tile ({tx},{ty}) 像素不一致");
                }
            }
        }
    }
    assert!(compared > 0, "{what}: 没有采到任何已分配 tile，测试无效");
    println!("  {what}: 逐像素比对通过（{compared} 个 tile 全等）");
}

/// 一条确定性的正弦笔画脚本（关掉按时间出 dab，结果只取决于事件流）。
fn sine_script(x0: f32, y0: f32, len: f32) -> Vec<(f32, f32, f32)> {
    (0..=50)
        .map(|i| {
            let t = i as f32 / 50.0;
            (
                x0 + t * len,
                y0 + (t * std::f32::consts::TAU).sin() * 18.0,
                0.3 + 0.7 * (t * std::f32::consts::PI).sin(),
            )
        })
        .collect()
}

fn stroke_brush() -> Brush {
    let mut b = Brush::new();
    let mut set = |s: BrushSetting, v: f32| b.get_mut(s).base_value = v;
    set(BrushSetting::Radius, 2.0); // ~7.4px
    set(BrushSetting::DabsPerBasicRadius, 2.0);
    set(BrushSetting::DabsPerActualRadius, 2.0);
    set(BrushSetting::DabsPerSecond, 0.0);
    set(BrushSetting::Hardness, 0.8);
    set(BrushSetting::Opaque, 0.8);
    set(BrushSetting::ColorH, 0.1);
    set(BrushSetting::ColorS, 0.9);
    set(BrushSetting::ColorV, 0.9);
    b
}

fn drive<S: TiledSurface>(surface: &mut S, brush: &Brush, script: &[(f32, f32, f32)]) {
    let mut state = BrushState::default();
    surface.begin_atomic();
    for &(x, y, p) in script {
        brush.stroke_to(&mut state, surface, x, y, p, 0.0, 0.0, 0.01);
    }
    surface.end_atomic();
}

// ---- ① 硬门禁：对称关闭 = 逐像素不变 ----

/// 对称关闭时，`draw_dab` 的覆写必须**完全透明**：像素与"从来不含对称代码"
/// 的参考后端逐位相同。
///
/// 覆盖两条路径：(a) 从未调过 `set_symmetry_state`（`SymmetryData::default()`
/// 已经算好了 16 个矩阵，但 `active == false`）；(b) 显式设成 `active = false`
/// 但把类型/中心/线数都设成非默认值（矩阵真的按 Snowflake 算过一遍）。
#[test]
fn symmetry_off_is_bit_identical_to_reference() {
    let brush = stroke_brush();
    let script = sine_script(20.0, 60.0, 150.0);

    // (c) 参考后端：hokusai 的 MemSurface（只有默认 draw_dab，没有对称）
    let mut reference = MemSurface::new();
    drive(&mut reference, &brush, &script);

    // (a) 从没碰过对称 API
    let mut pristine = PigmentSurface::new();
    drive(&mut pristine, &brush, &script);
    assert_matches_reference(&pristine, &reference, "对称从未启用");

    // (b) 显式关闭对称，但参数都设成"非默认"，且中心落在笔画上
    let mut disabled = PigmentSurface::new();
    disabled.set_symmetry_state(false, 60.0, 60.0, SymmetryType::Snowflake, 6, 30.0);
    drive(&mut disabled, &brush, &script);
    assert_matches_reference(&disabled, &reference, "对称显式关闭");
    assert_pigment_identical(&disabled, &pristine, "关闭态与原始态");
}

// ---- ② 镜像位置与镜像角度 ----

/// 垂直镜像（默认类型）：dab 画在轴右侧，左侧出现**镜像**的笔迹。
///
/// 判据全部可判定：
/// - 外接框横向关于 x=0 对称（半径 5 的圆 dab，中心在 ±20，实心圆占 10px）；
/// - 每个像素与它的镜像像素 alpha 相等（容差 1e-3，见下面注释）；
/// - 关闭对称时外接框只在右半边。
#[test]
fn vertical_mirror_places_ink_symmetrically() {
    let dab = test_dab(20.0, 0.0, 25.0, 1.0);

    let mut plain = PigmentSurface::new();
    paint(&mut plain, &[dab]);
    let b = ink_bbox(&plain).expect("关闭对称也该画出笔迹");
    assert!(b.0 > 0, "关闭对称时笔迹只在右半边：{b:?}");

    let mut mirrored = PigmentSurface::new();
    mirrored.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Vertical, 2, 0.0);
    paint(&mut mirrored, &[dab]);
    let b = ink_bbox(&mirrored).expect("开启对称后应有笔迹");
    assert!(b.0 <= -24 && b.0 >= -26, "左边界应镜像到 -25，实得 {b:?}");
    let right = b.0 + b.2 - 1;
    assert!(right >= 23 && right <= 25, "右边界应到 24 附近，实得 {b:?}");
    assert!(b.1 >= -6 && b.1 <= -4, "纵向位置不该被改：{b:?}");
    assert!(b.3 <= 12, "纵向高度不该被拉长：{b:?}");

    // 逐像素镜像：像素 (x, y) 的镜像是 (-x-1, y)。
    //
    // 为什么是 -x-1：dab 的采样点在像素**中心**（`brushmodes.rs` 的 `rr_at`
    // 用 `px + 0.5`），索引 x 的中心 x+0.5 关于 0 的镜像点是 -x-0.5，
    // 而那是索引 -x-1 的中心。
    //
    // 用容差而非全等 —— 镜像 dab 的角度是 -2*(90+0)-25 = -205°，
    // 与 25° 是不同输入，cos/sin 的末位 ulp 可能不同。
    let mut compared = 0;
    for x in -30..=30 {
        for y in -8..=8 {
            let (a, b) = (alpha_at(&mirrored, x, y), alpha_at(&mirrored, -x - 1, y));
            assert!(
                (a - b).abs() < 1e-3,
                "({x},{y}) 的 alpha {a} 与镜像位置 ({},{y}) 的 {b} 不等",
                -x - 1
            );
            if a > 0.01 {
                compared += 1;
            }
        }
    }
    assert!(compared > 40, "采到的笔迹像素太少：{compared}");
}

/// 水平镜像：dab 画在轴下方，上方出现镜像笔迹；外接框纵向对称。
#[test]
fn horizontal_mirror_places_ink_symmetrically() {
    let dab = test_dab(0.0, 20.0, 0.0, 1.0);
    let mut surface = PigmentSurface::new();
    surface.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Horizontal, 2, 0.0);
    paint(&mut surface, &[dab]);

    let b = ink_bbox(&surface).expect("应有笔迹");
    assert!(b.1 <= -24 && b.1 >= -26, "上边界应到 -25，实得 {b:?}");
    let bottom = b.1 + b.3 - 1;
    assert!(bottom >= 23 && bottom <= 25, "下边界应到 24，实得 {b:?}");
    assert!(b.0 >= -6 && b.0 <= -4, "横向位置不该被改：{b:?}");
    assert!(b.2 <= 12, "横向宽度不该被拉长：{b:?}");

    // 镜像位置确实有笔迹、且和原始位置一样深
    assert!(alpha_at(&surface, 0, -20) > 0.4, "(-20) 处应有镜像笔迹");
    assert!(alpha_at(&surface, 0, 20) > 0.4, "(+20) 处应有原始笔迹");
}

/// 在对称轴上画：镜像 dab 落回原地 → 外接框与单笔**相同**，
/// 但同一处被盖了两笔（alpha 明显更高）。
///
/// 这是"角度/反射正确"的可判定推论：椭圆 dab（aspect 3、倾斜 20°）的
/// 镜像是一个方向被反射的椭圆，外接框不变；如果角度公式算错、或者
/// 位置偏了，外接框一定会被撑大。
#[test]
fn dab_on_the_axis_reflects_onto_itself() {
    let dab = test_dab(0.0, 0.0, 20.0, 3.0);

    let mut single = PigmentSurface::new();
    paint(&mut single, &[dab]);
    let single_bbox = ink_bbox(&single).expect("单笔应有笔迹");

    let mut mirrored = PigmentSurface::new();
    mirrored.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Vertical, 2, 0.0);
    paint(&mut mirrored, &[dab]);
    let mirrored_bbox = ink_bbox(&mirrored).expect("对称后应有笔迹");

    assert_eq!(
        single_bbox, mirrored_bbox,
        "镜像 dab 落在对称轴上，外接框不该变"
    );
    // 双重盖章：中心 alpha 明显高于单笔（0.5 + 0.5*0.5 = 0.75）
    let a_single = alpha_at(&single, 0, 0);
    let a_mirrored = alpha_at(&mirrored, 0, 0);
    assert!(
        a_mirrored > a_single + 0.1,
        "轴上同一处应被盖两笔：单笔 {a_single}，对称 {a_mirrored}"
    );
}

// ---- ③ 旋转对称的份数 ----

/// 旋转对称 N 条线：原始 dab + N-1 个旋转 dab，落在 N 等分角上。
#[test]
fn rotational_lands_on_n_equal_sectors() {
    for lines in [3i32, 4, 6] {
        let (cx, cy, r) = (32.0f32, 32.0f32, 18.0f32);
        let mut surface = PigmentSurface::new();
        surface.set_symmetry_state(true, cx, cy, SymmetryType::Rotational, lines, 0.0);
        paint(&mut surface, &[test_dab(cx + r, cy, 0.0, 1.0)]);

        // 每个 N 等分角上都应有笔迹
        for k in 0..lines {
            let a = std::f32::consts::TAU * k as f32 / lines as f32;
            let x = (cx + r * a.cos()).round() as i32;
            let y = (cy + r * a.sin()).round() as i32;
            assert!(
                alpha_at(&surface, x, y) > 0.05,
                "{lines} 条线：({x},{y}) 应有第 {k} 份 dab，alpha={}",
                alpha_at(&surface, x, y)
            );
        }
        // 相邻两份之间的中间角上不该有笔迹（只在 3/4 条线时成立：
        // 6 条线的相邻 dab 中心只隔 2*r*sin(15°)≈9.3px，中点离圆心的
        // 距离会落进半径 5 的 dab 里）
        if lines <= 4 {
            let a = std::f32::consts::TAU / (2.0 * lines as f32);
            let x = (cx + r * a.cos()).round() as i32;
            let y = (cy + r * a.sin()).round() as i32;
            assert!(
                alpha_at(&surface, x, y) < 0.01,
                "{lines} 条线：({x},{y}) 在两个 dab 之间，不该有笔迹，alpha={}",
                alpha_at(&surface, x, y)
            );
        }
    }
}

/// 雪花 = 旋转 + 每条线的镜像。3 条线 → 额外 2*n-1 = 5 个 dab。
///
/// 期望位置来自源码公式（`mypaint-symmetry.c:101-112` 的矩阵 +
/// `mypaint-tiled-surface.c:755-786` 的展开），这里当金标准钉住：
/// 原始 dab 在 (20,5)，5 个对称 dab 落在
/// `(20,-5) (-14.33,-14.82) (-5.67,19.82) (-5.67,-19.82) (-14.33,14.82)`。
#[test]
fn snowflake_adds_mirrored_dabs() {
    let mut surface = PigmentSurface::new();
    surface.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Snowflake, 3, 0.0);
    paint(&mut surface, &[test_dab(20.0, 5.0, 0.0, 1.0)]);

    let expected = [
        (20.0f32, 5.0f32),      // 原始 dab
        (20.0, -5.0),           // 反射
        (-14.33, -14.82),       // 反射
        (-5.67, 19.82),         // 反射
        (-5.67, -19.82),        // 旋转
        (-14.33, 14.82),        // 旋转
    ];
    for (x, y) in expected {
        assert!(
            alpha_at(&surface, x.round() as i32, y.round() as i32) > 0.05,
            "({x},{y}) 应有笔迹，实得 alpha={}",
            alpha_at(&surface, x.round() as i32, y.round() as i32)
        );
    }
    // 集合对 y → -y 封闭（雪花的每条轴都带镜像）
    for (x, y) in expected {
        let mirrored = alpha_at(&surface, x.round() as i32, (-y).round() as i32);
        assert!(
            mirrored > 0.05,
            "({x},{}) 应镜像存在，实得 alpha={mirrored}",
            -y
        );
    }
    // 中心不该有笔迹（5 个对称 dab 的中心离原点都有 20px）
    assert!(alpha_at(&surface, 0, 0) < 0.01, "对称中心不该被涂到");
}

// ---- ④ 中心与角度的平移 ----

/// 对称中心与对称线角度都真的参与计算。
#[test]
fn center_and_angle_transform_the_result() {
    // 中心 (100, 50)，dab 在 (110, 40)：
    // 垂直轴（angle=0）→ 镜像到 (90, 40)
    let mut vertical = PigmentSurface::new();
    vertical.set_symmetry_state(true, 100.0, 50.0, SymmetryType::Vertical, 2, 0.0);
    paint(&mut vertical, &[test_dab(110.0, 40.0, 0.0, 1.0)]);
    assert!(alpha_at(&vertical, 90, 40) > 0.4, "应镜像到 (90,40)");
    assert!(alpha_at(&vertical, 110, 40) > 0.4, "原始 dab 在 (110,40)");

    // 同一中心、角度 90°：垂直轴的镜像变成水平轴的镜像 → (110, 60)
    let mut turned = PigmentSurface::new();
    turned.set_symmetry_state(true, 100.0, 50.0, SymmetryType::Vertical, 2, 90.0);
    paint(&mut turned, &[test_dab(110.0, 40.0, 0.0, 1.0)]);
    assert!(
        alpha_at(&turned, 110, 60) > 0.4,
        "角度 90° 后应镜像到 (110,60)，实得 alpha={}",
        alpha_at(&turned, 110, 60)
    );
    assert!(alpha_at(&turned, 90, 40) < 0.01, "不该再出现垂直镜像位置");

    // 中心挪到原点 → 镜像落到 (-110, 40)
    let mut moved = PigmentSurface::new();
    moved.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Vertical, 2, 0.0);
    paint(&mut moved, &[test_dab(110.0, 40.0, 0.0, 1.0)]);
    assert!(alpha_at(&moved, -110, 40) > 0.4, "中心在原点时镜像到 (-110,40)");
    assert!(alpha_at(&moved, 90, 40) < 0.01, "中心挪走后旧镜像位置应为空");
}

// ---- ⑤ 退化 dab：源码的早期返回判据也管住对称 pass ----

/// 源码用 `draw_dab_internal` 的早期返回（`mypaint-tiled-surface.c:642-645`）
/// 决定要不要做对称 pass。半径 < 0.1 的 dab 连对称 dab 都不该出现。
#[test]
fn degenerate_dab_draws_no_symmetry_copy() {
    let mut surface = PigmentSurface::new();
    surface.set_symmetry_state(true, 0.0, 0.0, SymmetryType::Vertical, 2, 0.0);
    let mut dab = test_dab(20.0, 0.0, 0.0, 1.0);
    dab.radius = 0.05; // < 0.1 → 源码直接 return FALSE
    paint(&mut surface, &[dab]);
    assert!(
        ink_bbox(&surface).is_none(),
        "半径 0.05 的 dab 不该画出任何东西"
    );

    // 对照：同样位置、半径 5 就会画（并且镜像）
    paint(&mut surface, &[test_dab(20.0, 0.0, 0.0, 1.0)]);
    assert!(alpha_at(&surface, -20, 0) > 0.1, "正常 dab 应有镜像");
}
