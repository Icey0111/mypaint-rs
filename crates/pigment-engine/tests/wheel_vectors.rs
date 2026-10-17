//! 色轮 / 色方块 / 色立方 / HCY 轮的向量逐位门禁（B4-4b / B4-4c）。
//!
//! fixture：`tests/fixtures/wheel_vectors.txt`（6092 条向量，其中 `hcy_*`
//! 1708 条），由 `tests/tools/gen-wheel-vectors.py` 生成：
//!
//! ```text
//! python crates/pigment-engine/tests/tools/gen-wheel-vectors.py
//! ```
//!
//! 脚本 `import` 的是**真** `lib/color.py`、`lib/alg.py` 与
//! `gui/colors/util.py`，并用 `ast` 把 `adjbases.py` / `hsvwheel.py` /
//! `hsvsquare.py` / `hsvcube.py` / `hcywheel.py` / `sliders.py` 里纯数学的
//! 方法体抽出来 `exec` —— 执行的是源码那几行，不是照抄的公式（每个被抽方法的
//! 行号在脚本运行时打印）。浮点一律 Python `repr()`（最短往返），所以这里
//! **零容差**逐位比对（`to_bits()`）；只有 `+0.0`/`-0.0` 视为相等。
//!
//! 依赖 GTK 控件状态的部分（按下/抬起状态机、`queue_draw`、`get_allocation`
//! 的真实分配）不在这份 fixture 里：控件的几何/命中在
//! `crates/pigment-app/src/shell/wheel_pages.rs` 与
//! `.../hcy_wheel_page.rs` 的测试里用源码行号手推期望值。

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::OnceLock;

use pigment_engine::color::{
    colors_to_mask_void, convex_hull, depth_slider_amount, depth_slider_color,
    distance_fade_stops, distort_hue, editor_luma_y, hcy_luma_slider_samples,
    hcy_wheel_color_at_position, hcy_wheel_position_for_color, hcy_wheel_scroll,
    hsv_ring_color_at_position, hsv_wheel_color_at_position, hsv_wheel_position_for_color,
    hsv_wheel_scroll, hue_from_angle, mask_templates, mask_voids, masked_color_at_position,
    nearest_point_in_segment, point_in_convex_poly, poly_area, saturation_from_radius,
    slice_color_at_position, slice_faces, slice_position_for_color, template_mask, tumble_faces,
    undistort_hue, visible_mask_voids, void_size, Color, HcyColor, HsvColor, HsvFace, HueRange,
    MaskShape, Point, ScrollDirection, WheelGeometry, WheelType, CUBE_SLIDER_SAMPLES,
    EDGE_HIGHLIGHT_RGBA, EDGE_HIGHLIGHT_WIDTH, HUE_SLICES, MASK_DRAW_ALPHA, MASK_EDGE_LINE_WIDTH,
    MIN_SHAPE_SIZE, OUTLINE_RGBA, OUTLINE_WIDTH, PRIMARY_ADJUSTERS_MIN_HEIGHT,
    PRIMARY_ADJUSTERS_MIN_WIDTH, RING_SAMPLES, SAT_GAMMA, SAT_SLICES, SLIDER_MIN_LENGTH,
    SLIDER_MIN_WIDTH,
};

// ---------------------------------------------------------------------------
// fixture 读取
// ---------------------------------------------------------------------------

fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/wheel_vectors.txt",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn fixture() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        std::fs::read_to_string(fixture_path()).unwrap_or_else(|e| {
            panic!(
                "读不到 fixture {}：{e}（先跑 tests/tools/gen-wheel-vectors.py）",
                fixture_path()
            )
        })
    })
}

fn data_lines() -> impl Iterator<Item = (usize, &'static str)> {
    fixture()
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
}

/// 某类行，返回 `(行号, 字段)`（字段 0 = 行类型）。
fn rows(kind: &'static str) -> impl Iterator<Item = (usize, Vec<&'static str>)> {
    data_lines().filter_map(move |(ln, line)| {
        let f: Vec<&'static str> = line.split('\t').collect();
        (f.first() == Some(&kind)).then_some((ln, f))
    })
}

fn f64_of(tok: &str, ln: usize) -> f64 {
    tok.parse::<f64>()
        .unwrap_or_else(|e| panic!("第 {ln} 行不是浮点：{tok:?}（{e}）"))
}

fn bool_of(tok: &str, ln: usize) -> bool {
    match tok {
        "0" => false,
        "1" => true,
        other => panic!("第 {ln} 行不是 0/1：{other:?}"),
    }
}

fn wheel_type_of(tok: &str, ln: usize) -> WheelType {
    WheelType::from_name(tok).unwrap_or_else(|| panic!("第 {ln} 行未知轮类型：{tok:?}"))
}

fn face_of(tok: &str, ln: usize) -> HsvFace {
    HsvFace::from_name(tok).unwrap_or_else(|| panic!("第 {ln} 行未知面：{tok:?}"))
}

fn faces_of(tok: &str, ln: usize) -> [HsvFace; 3] {
    let chars: Vec<char> = tok.chars().collect();
    assert_eq!(chars.len(), 3, "第 {ln} 行的面表应有三项：{tok:?}");
    [
        face_of(&chars[0].to_string(), ln),
        face_of(&chars[1].to_string(), ln),
        face_of(&chars[2].to_string(), ln),
    ]
}

fn hsv_of(toks: &[&str], ln: usize) -> HsvColor {
    assert_eq!(toks.len(), 3, "第 {ln} 行应有 3 个 HSV 分量");
    HsvColor::new(
        f64_of(toks[0], ln),
        f64_of(toks[1], ln),
        f64_of(toks[2], ln),
    )
}

/// 逐位相等（`+0.0` 与 `-0.0` 视为相等）。
fn same_bits(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits() || (actual == 0.0 && expected == 0.0)
}

#[track_caller]
fn assert_bits(actual: f64, expected: f64, ctx: &str) {
    assert!(
        same_bits(actual, expected),
        "{ctx} 不一致：实际 {actual:?}（bits {:#x}）期望 {expected:?}（bits {:#x}）",
        actual.to_bits(),
        expected.to_bits()
    );
}

#[track_caller]
fn assert_hsv(actual: HsvColor, expected: HsvColor, ctx: &str) {
    assert_bits(actual.h, expected.h, &format!("{ctx} 的 h"));
    assert_bits(actual.s, expected.s, &format!("{ctx} 的 s"));
    assert_bits(actual.v, expected.v, &format!("{ctx} 的 v"));
}

// ---------------------------------------------------------------------------
// 常量 / 畸变表
// ---------------------------------------------------------------------------

/// 类常量与模块常量 —— `adjbases.py:449-461` / `:1105-1114`、
/// `uimisc.py:21-26`、`hsvsquare.py:142`、`hsvcube.py:122`。
#[test]
fn constants_match_source() {
    let mut seen = BTreeSet::new();
    for (ln, f) in rows("wh_const") {
        let name = f[1];
        let value = f[2];
        seen.insert(name.to_string());
        match name {
            "HUE_SLICES" => assert_eq!(f64_of(value, ln) as usize, HUE_SLICES),
            "SAT_SLICES" => assert_eq!(f64_of(value, ln) as usize, SAT_SLICES),
            "SAT_GAMMA" => assert_bits(f64_of(value, ln), SAT_GAMMA, "SAT_GAMMA"),
            "BORDER_WIDTH" => assert_bits(
                f64_of(value, ln),
                pigment_engine::color::BORDER_WIDTH,
                "BORDER_WIDTH",
            ),
            "SCROLL_DELTA" => assert_bits(
                f64_of(value, ln),
                pigment_engine::color::SCROLL_DELTA,
                "SCROLL_DELTA",
            ),
            "OUTLINE_WIDTH" => assert_bits(f64_of(value, ln), OUTLINE_WIDTH, "OUTLINE_WIDTH"),
            "EDGE_HIGHLIGHT_WIDTH" => assert_bits(
                f64_of(value, ln),
                EDGE_HIGHLIGHT_WIDTH,
                "EDGE_HIGHLIGHT_WIDTH",
            ),
            "PRIMARY_ADJUSTERS_MIN_WIDTH" => assert_bits(
                f64_of(value, ln),
                PRIMARY_ADJUSTERS_MIN_WIDTH,
                "PRIMARY_ADJUSTERS_MIN_WIDTH",
            ),
            "PRIMARY_ADJUSTERS_MIN_HEIGHT" => assert_bits(
                f64_of(value, ln),
                PRIMARY_ADJUSTERS_MIN_HEIGHT,
                "PRIMARY_ADJUSTERS_MIN_HEIGHT",
            ),
            "SLIDER_MIN_WIDTH" => {
                assert_bits(f64_of(value, ln), SLIDER_MIN_WIDTH, "SLIDER_MIN_WIDTH")
            }
            "SLIDER_MIN_LENGTH" => {
                assert_bits(f64_of(value, ln), SLIDER_MIN_LENGTH, "SLIDER_MIN_LENGTH")
            }
            "RING_SAMPLES" => assert_eq!(f64_of(value, ln) as usize, RING_SAMPLES),
            "CUBE_SLIDER_SAMPLES" => {
                assert_eq!(f64_of(value, ln) as usize, CUBE_SLIDER_SAMPLES)
            }
            "DEFAULT_WHEEL_TYPE" => {
                assert_eq!(value, pigment_engine::color::DEFAULT_WHEEL_TYPE)
            }
            "PREFS_KEY_WHEEL_TYPE" => {
                assert_eq!(value, pigment_engine::color::PREFS_KEY_WHEEL_TYPE)
            }
            other => panic!("第 {ln} 行有未断言的常量 {other:?}"),
        }
    }
    assert_eq!(seen.len(), 15, "fixture 的常量表少了几条：{seen:?}");

    for (ln, f) in rows("wh_rgba") {
        let rgba = [
            f64_of(f[2], ln),
            f64_of(f[3], ln),
            f64_of(f[4], ln),
            f64_of(f[5], ln),
        ];
        let expect = match f[1] {
            "OUTLINE_RGBA" => OUTLINE_RGBA,
            "EDGE_HIGHLIGHT_RGBA" => EDGE_HIGHLIGHT_RGBA,
            other => panic!("第 {ln} 行未知颜色常量 {other:?}"),
        };
        for (i, (a, e)) in rgba.iter().zip(expect.iter()).enumerate() {
            assert_bits(*a, *e, &format!("{} 分量 {i}", f[1]));
        }
    }
}

/// 三张色相畸变表 —— `adjbases.py:80-95`。
#[test]
fn hue_distortion_tables_match_source() {
    let mut seen = BTreeSet::new();
    for (ln, f) in rows("wh_table") {
        let wheel_type = wheel_type_of(f[1], ln);
        let segments = f64_of(f[2], ln) as usize;
        seen.insert(wheel_type.name());
        match wheel_type.ranges() {
            None => assert_eq!(segments, 0, "第 {ln} 行：rgb 的表应为空"),
            Some(ranges) => {
                assert_eq!(ranges.len(), segments, "第 {ln} 行段数");
                for (i, &((in0, in1), (out0, out1))) in ranges.iter().enumerate() {
                    let base = 3 + i * 4;
                    assert_bits(f64_of(f[base], ln), in0, "in0");
                    assert_bits(f64_of(f[base + 1], ln), in1, "in1");
                    assert_bits(f64_of(f[base + 2], ln), out0, "out0");
                    assert_bits(f64_of(f[base + 3], ln), out1, "out1");
                }
            }
        }
    }
    assert_eq!(seen.len(), 3, "fixture 里应有 rgb / ryb / rygb 三种轮");
}

/// 色相畸变与逆畸变 —— `adjbases.py:317-345`。
#[test]
fn distort_and_undistort_match_source() {
    let mut n = 0;
    for (ln, f) in rows("wh_distort") {
        let wheel_type = wheel_type_of(f[1], ln);
        let h = f64_of(f[2], ln);
        let expect = f64_of(f[3], ln);
        assert_bits(
            distort_hue(h, wheel_type.ranges()),
            expect,
            &format!("第 {ln} 行 distort {h}"),
        );
        n += 1;
    }
    assert!(n >= 50, "畸变向量太少：{n}");
    for (ln, f) in rows("wh_undistort") {
        let wheel_type = wheel_type_of(f[1], ln);
        let h = f64_of(f[2], ln);
        let expect = f64_of(f[3], ln);
        assert_bits(
            undistort_hue(h, wheel_type.ranges()),
            expect,
            &format!("第 {ln} 行 undistort {h}"),
        );
    }
}

// ---------------------------------------------------------------------------
// 几何
// ---------------------------------------------------------------------------

/// 半径与圆心 —— `adjbases.py:1116-1136`。
#[test]
fn wheel_geometry_matches_source() {
    for (ln, f) in rows("wh_geom") {
        let geom = WheelGeometry::for_size(f64_of(f[1], ln), f64_of(f[2], ln), f64_of(f[3], ln));
        assert_bits(geom.radius, f64_of(f[4], ln), &format!("第 {ln} 行半径"));
        assert_bits(geom.center_x, f64_of(f[5], ln), &format!("第 {ln} 行圆心 x"));
        assert_bits(geom.center_y, f64_of(f[6], ln), &format!("第 {ln} 行圆心 y"));
    }
}

/// 位置 → `(半径比, 色相角)`（含 `SAT_GAMMA` 与逆畸变）——
/// `adjbases.py:1148-1167`。
#[test]
fn polar_from_position_matches_source() {
    let mut n = 0;
    for (ln, f) in rows("wh_polar") {
        let wheel_type = wheel_type_of(f[1], ln);
        let geom = WheelGeometry::for_size(f64_of(f[2], ln), f64_of(f[3], ln), 2.0);
        let x = f64_of(f[4], ln);
        let y = f64_of(f[5], ln);
        let (r, theta) = geom
            .polar_at(x, y, SAT_GAMMA)
            .unwrap_or_else(|| panic!("第 {ln} 行：几何应可用"));
        assert_bits(r, f64_of(f[6], ln), &format!("第 {ln} 行 r"));
        assert_bits(
            undistort_hue(theta, wheel_type.ranges()),
            f64_of(f[7], ln),
            &format!("第 {ln} 行 theta"),
        );
        // 再验一次 hue_from_angle 直接算的原始角度（不经 mixin 的假 manager）
        assert_bits(
            hue_from_angle(x - geom.center_x, y - geom.center_y),
            theta,
            &format!("第 {ln} 行 hue_from_angle"),
        );
        n += 1;
    }
    assert!(n >= 50, "极坐标向量太少：{n}");
}

/// 半径 → 饱和度的独立校验（`adjbases.py:1158`）：
/// `wheel_radius` 的整数化与 `SAT_GAMMA` 的幂。
#[test]
fn saturation_gamma_round_trips_marker_positions() {
    let geom = WheelGeometry::for_size(100.0, 100.0, 2.0);
    for frac in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
        let r = geom.radius * frac;
        let s = saturation_from_radius(r, geom.radius, SAT_GAMMA).unwrap();
        assert_bits(
            s,
            frac.powf(SAT_GAMMA),
            &format!("半径比 {frac} 的饱和度"),
        );
        // 画标记是逆运算，四舍五入到像素后应回到原处（±1px）
        let (x, _) = geom.position_for(s, 0.0, SAT_GAMMA);
        assert!((x - (geom.center_x + r)).abs() <= 1.0, "frac={frac} x={x}");
    }
}

// ---------------------------------------------------------------------------
// 两种色轮的钩子
// ---------------------------------------------------------------------------

/// 取色：位置 → 颜色 —— `adjbases.py:1148-1167` + `hsvwheel.py:53-57` /
/// `hsvsquare.py:189-192`。
#[test]
fn wheel_and_ring_colors_match_source() {
    let mut kinds = BTreeSet::new();
    for (ln, f) in rows("wh_color") {
        let kind = f[1];
        kinds.insert(kind.to_string());
        let wheel_type = wheel_type_of(f[2], ln);
        let geom = WheelGeometry::for_size(f64_of(f[3], ln), f64_of(f[4], ln), 2.0);
        let x = f64_of(f[5], ln);
        let y = f64_of(f[6], ln);
        let managed = Color::Hsv(hsv_of(&f[7..10], ln));
        let expected = hsv_of(&f[10..13], ln);
        let actual = match kind {
            "wheel" => hsv_wheel_color_at_position(&geom, x, y, &managed, wheel_type.ranges()),
            "ring" => hsv_ring_color_at_position(&geom, x, y, &managed, wheel_type.ranges()),
            other => panic!("第 {ln} 行未知 kind {other:?}"),
        }
        .unwrap_or_else(|| panic!("第 {ln} 行：几何应可用"));
        assert_hsv(actual, expected, &format!("第 {ln} 行 {kind} 取色"));
    }
    assert_eq!(kinds.len(), 2, "wheel 与 ring 两种钩子都要覆盖");
}

/// 标记位置：颜色 → 像素 —— `adjbases.py:1301-1315`。
#[test]
fn wheel_and_ring_marker_positions_match_source() {
    for (ln, f) in rows("wh_pos") {
        let kind = f[1];
        let wheel_type = wheel_type_of(f[2], ln);
        let geom = WheelGeometry::for_size(f64_of(f[3], ln), f64_of(f[4], ln), 2.0);
        let color = Color::Hsv(hsv_of(&f[5..8], ln));
        let (x, y) = match kind {
            "wheel" => hsv_wheel_position_for_color(&geom, &color, wheel_type.ranges()),
            "ring" => {
                pigment_engine::color::hsv_ring_position_for_color(&geom, &color, wheel_type.ranges())
            }
            other => panic!("第 {ln} 行未知 kind {other:?}"),
        };
        assert_bits(x, f64_of(f[8], ln), &format!("第 {ln} 行 {kind} 标记 x"));
        assert_bits(y, f64_of(f[9], ln), &format!("第 {ln} 行 {kind} 标记 y"));
    }
}

// ---------------------------------------------------------------------------
// 滚轮 / 渐变
// ---------------------------------------------------------------------------

/// 轮上滚轮 —— `hsvwheel.py:35-47`。
#[test]
fn wheel_scroll_matches_source() {
    for (ln, f) in rows("wh_scroll") {
        let v0 = f64_of(f[1], ln);
        let direction = match f[2] {
            "UP" => ScrollDirection::Up,
            "DOWN" => ScrollDirection::Down,
            "LEFT" => ScrollDirection::Left,
            "RIGHT" => ScrollDirection::Right,
            "SMOOTH" => ScrollDirection::Smooth,
            other => panic!("第 {ln} 行未知方向 {other:?}"),
        };
        let changed = bool_of(f[3], ln);
        let v1 = f64_of(f[4], ln);
        let managed = Color::Hsv(HsvColor::new(0.3, 0.4, v0));
        match hsv_wheel_scroll(&managed, direction) {
            None => assert!(!changed, "第 {ln} 行：源码的 if 不成立时不应落色"),
            Some(Color::Hsv(col)) => {
                assert!(changed, "第 {ln} 行：应落色");
                assert_bits(col.v, v1, &format!("第 {ln} 行明度"));
                assert_bits(col.h, 0.3, "色相不该动");
                assert_bits(col.s, 0.4, "饱和度不该动");
            }
            Some(other) => panic!("第 {ln} 行：滚轮应回写 HSVColor，实得 {}", other.class_name()),
        }
    }
}

/// 径向去饱和渐变色标 —— `util.py:27-40`。
#[test]
fn fade_stops_match_source() {
    use std::collections::BTreeMap;

    let mut metas: BTreeMap<usize, (usize, f64, f64, usize)> = BTreeMap::new();
    for (ln, f) in rows("wh_fade_meta") {
        let case = f64_of(f[1], ln) as usize;
        metas.insert(
            case,
            (
                f64_of(f[2], ln) as usize,
                f64_of(f[3], ln),
                f64_of(f[4], ln),
                f64_of(f[5], ln) as usize,
            ),
        );
    }
    assert!(!metas.is_empty(), "fixture 里没有渐变向量");

    let mut stops: BTreeMap<usize, Vec<(f64, [f64; 4])>> = BTreeMap::new();
    for (ln, f) in rows("wh_fade_stop") {
        let case = f64_of(f[1], ln) as usize;
        stops.entry(case).or_default().push((
            f64_of(f[3], ln),
            [
                f64_of(f[4], ln),
                f64_of(f[5], ln),
                f64_of(f[6], ln),
                f64_of(f[7], ln),
            ],
        ));
    }

    for (case, (nstops, gamma, alpha, count)) in metas {
        let got = stops.remove(&case).unwrap_or_default();
        assert_eq!(got.len(), count, "case {case} 的色标数");
        // 用 fixture 里的第一个色标反推 rgb（源码把 rgb 原样写进每个色标）
        let rgb = [got[0].1[0], got[0].1[1], got[0].1[2]];
        let actual = distance_fade_stops(rgb, nstops, gamma, alpha);
        assert_eq!(actual.len(), got.len(), "case {case}");
        for (i, ((a_stop, a_rgba), (e_stop, e_rgba))) in
            actual.iter().zip(got.iter()).enumerate()
        {
            assert_bits(*a_stop, *e_stop, &format!("case {case} 第 {i} 个色标位置"));
            for c in 0..4 {
                assert_bits(a_rgba[c], e_rgba[c], &format!("case {case} 第 {i} 个色标分量 {c}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 方块 / 立方切片
// ---------------------------------------------------------------------------

/// 切片坐标 → HSV —— `hsvsquare.py:382-396` / `hsvcube.py:213-227`。
#[test]
fn slice_color_matches_source() {
    let mut pages = BTreeSet::new();
    for (ln, f) in rows("wh_slice_color") {
        pages.insert(f[1].to_string());
        let faces = faces_of(f[2], ln);
        let width = f64_of(f[3], ln);
        let height = f64_of(f[4], ln);
        let x = f64_of(f[5], ln);
        let y = f64_of(f[6], ln);
        let managed = Color::Hsv(hsv_of(&f[7..10], ln));
        let expected = hsv_of(&f[10..13], ln);
        let actual = slice_color_at_position(&managed, x, y, width, height, 2.0, faces)
            .unwrap_or_else(|| panic!("第 {ln} 行：有效尺寸应为正"));
        assert_hsv(actual, expected, &format!("第 {ln} 行 {} 切片取色", f[1]));
    }
    assert_eq!(pages.len(), 2, "square 与 cube 两种页面都要覆盖");
}

/// 切片标记位置 —— `hsvsquare.py:398-412` / `hsvcube.py:229-243`。
#[test]
fn slice_position_matches_source() {
    for (ln, f) in rows("wh_slice_pos") {
        let faces = faces_of(f[2], ln);
        let width = f64_of(f[3], ln);
        let height = f64_of(f[4], ln);
        let color = Color::Hsv(hsv_of(&f[5..8], ln));
        let (x, y) = slice_position_for_color(&color, width, height, 2.0, faces);
        assert_bits(x, f64_of(f[8], ln), &format!("第 {ln} 行标记 x"));
        assert_bits(y, f64_of(f[9], ln), &format!("第 {ln} 行标记 y"));
    }
}

/// `__get_faces()` 的面交换 —— `hsvsquare.py:324-329` / `hsvcube.py:155-160`。
#[test]
fn slice_face_selection_matches_source() {
    for (ln, f) in rows("wh_faces") {
        let faces = faces_of(f[2], ln);
        let (f1, f2) = slice_faces(faces);
        assert_eq!(f1.name(), f[3], "第 {ln} 行 f1");
        assert_eq!(f2.name(), f[4], "第 {ln} 行 f2");
    }
}

/// 立方旋转 —— `hsvcube.py:100-105`。
#[test]
fn tumble_matches_source() {
    for (ln, f) in rows("wh_tumble") {
        let start = faces_of(f[1], ln);
        let step = f64_of(f[2], ln) as usize;
        let expected = faces_of(f[3], ln);
        let mut faces = start;
        for _ in 0..step {
            tumble_faces(&mut faces);
        }
        assert_eq!(faces, expected, "第 {ln} 行：{start:?} 转 {step} 次");
    }
}

// ---------------------------------------------------------------------------
// 立方深度滑条
// ---------------------------------------------------------------------------

/// 深度滑条的条值与颜色 —— `hsvcube.py:133-142`。
#[test]
fn depth_slider_matches_source() {
    for (ln, f) in rows("wh_slider_amt") {
        let face0 = face_of(f[1], ln);
        let managed = Color::Hsv(hsv_of(&f[2..5], ln));
        assert_bits(
            depth_slider_amount(&managed, face0),
            f64_of(f[5], ln),
            &format!("第 {ln} 行条值"),
        );
    }
    for (ln, f) in rows("wh_slider_color") {
        let face0 = face_of(f[1], ln);
        let managed = Color::Hsv(hsv_of(&f[2..5], ln));
        let amount = f64_of(f[5], ln);
        let expected = hsv_of(&f[6..9], ln);
        match depth_slider_color(&managed, face0, amount) {
            Color::Hsv(col) => assert_hsv(col, expected, &format!("第 {ln} 行条值取色")),
            other => panic!("第 {ln} 行应回写 HSVColor，实得 {}", other.class_name()),
        }
    }
}

/// 一个 `HueRange` 的形态（编译期类型约束，防止 fixture 与实现漂移）。
#[test]
fn hue_range_is_a_pair_of_pairs() {
    let range: HueRange = ((0.0, 1.0), (0.0, 0.5));
    assert_eq!(range.0 .0, 0.0);
}

// ---------------------------------------------------------------------------
// B4-4c：HCY 轮 + 色域遮罩（`hcy_*` 行）
// ---------------------------------------------------------------------------

/// 从字段切片里读 `n` 个点（每点两个浮点）。
fn points_of(toks: &[&str], n: usize, ln: usize) -> Vec<Point> {
    assert!(
        toks.len() >= 2 * n,
        "第 {ln} 行要 {n} 个点（{} 个浮点），只有 {} 个字段",
        2 * n,
        toks.len()
    );
    (0..n)
        .map(|i| (f64_of(toks[2 * i], ln), f64_of(toks[2 * i + 1], ln)))
        .collect()
}

/// `hcy_mask` 里的一段：`<颜色数> <h> <c> <y>…`（会前移游标）。
fn shape_at(f: &[&str], cursor: &mut usize, ln: usize) -> MaskShape {
    let n = f64_of(f[*cursor], ln) as usize;
    *cursor += 1;
    let mut shape = Vec::with_capacity(n);
    for _ in 0..n {
        shape.push(Color::Hcy(HcyColor::new(
            f64_of(f[*cursor], ln),
            f64_of(f[*cursor + 1], ln),
            f64_of(f[*cursor + 2], ln),
        )));
        *cursor += 3;
    }
    shape
}

/// 一个遮罩用例（`hcy_mask` 行）。
struct MaskCase {
    wheel_type: WheelType,
    width: f64,
    height: f64,
    mask: Vec<MaskShape>,
}

/// fixture 里所有 `hcy_mask` 行，按 tag 索引。
fn mask_cases() -> BTreeMap<String, MaskCase> {
    let mut out = BTreeMap::new();
    for (ln, f) in rows("hcy_mask") {
        let wheel_type = wheel_type_of(f[2], ln);
        let n_shapes = f64_of(f[5], ln) as usize;
        let mut cursor = 6;
        let mut mask = Vec::with_capacity(n_shapes);
        for _ in 0..n_shapes {
            mask.push(shape_at(&f, &mut cursor, ln));
        }
        assert_eq!(cursor, f.len(), "第 {ln} 行有剩余字段没读：{f:?}");
        out.insert(
            f[1].to_string(),
            MaskCase {
                wheel_type,
                width: f64_of(f[3], ln),
                height: f64_of(f[4], ln),
                mask,
            },
        );
    }
    assert!(!out.is_empty(), "fixture 里没有 hcy_mask 行");
    out
}

/// 常量（`hcywheel.py:43-44`、`:67`、`:79-90`、`:303-311`、`:344-347`、`:390-396`）。
#[test]
fn hcy_constants_match_source() {
    let mut seen = BTreeSet::new();
    for (ln, f) in rows("hcy_const") {
        let name = f[1];
        seen.insert(name.to_string());
        let text = f[2];
        let num = || f64_of(text, ln);
        match name {
            "MIN_SHAPE_SIZE" => assert_bits(num(), MIN_SHAPE_SIZE, "hcywheel.py:67"),
            "MIN_LUMA" => assert_bits(
                num(),
                pigment_engine::color::EDITOR_MIN_LUMA,
                "hcywheel.py:391",
            ),
            "MAX_LUMA" => assert_bits(
                num(),
                pigment_engine::color::EDITOR_MAX_LUMA,
                "hcywheel.py:390",
            ),
            "CTRLPOINT_RADIUS" => assert_bits(
                num(),
                pigment_engine::color::EDITOR_CTRLPOINT_RADIUS,
                "hcywheel.py:394",
            ),
            "CTRLPOINT_GRAB_RADIUS" => assert_bits(
                num(),
                pigment_engine::color::EDITOR_CTRLPOINT_GRAB_RADIUS,
                "hcywheel.py:395",
            ),
            "MAX_NUM_SHAPES" => assert_eq!(
                num() as usize,
                pigment_engine::color::EDITOR_MAX_NUM_SHAPES,
                "hcywheel.py:396"
            ),
            "MASK_EDGE_LINE_WIDTH" => {
                assert_bits(num(), MASK_EDGE_LINE_WIDTH, "hcywheel.py:303")
            }
            "MASK_DRAW_ALPHA" => assert_bits(num(), MASK_DRAW_ALPHA, "hcywheel.py:311"),
            "PREFS_MASK_KEY" => {
                assert_eq!(text, pigment_engine::color::PREFS_MASK_KEY, ":43");
            }
            "PREFS_ACTIVE_KEY" => {
                assert_eq!(text, pigment_engine::color::PREFS_ACTIVE_KEY, ":44");
            }
            "WHEEL_TOOLTIP" => {
                assert_eq!(text, pigment_engine::color::HCY_WHEEL_TOOLTIP, ":344-347");
            }
            "MASK_ACTIVE_LABEL" => {
                assert_eq!(text, pigment_engine::color::MASK_ACTIVE_LABEL, ":79-90");
            }
            "MASK_ACTIVE_TOOLTIP" => {
                assert_eq!(text, pigment_engine::color::MASK_ACTIVE_TOOLTIP, ":85-88");
            }
            "EDITOR_TOOLTIP" => {
                assert_eq!(text, pigment_engine::color::MASK_EDITOR_TOOLTIP, ":400-405");
            }
            other => panic!("第 {ln} 行有未断言的常量 {other:?}"),
        }
    }
    assert_eq!(seen.len(), 14, "常量表少了几条：{seen:?}");
}

/// 五个 mood 预设的名字/说明/形状数/每个点的 (h, c, y) ——
/// `hcywheel.py:868-988`。
#[test]
fn hcy_mask_templates_match_source() {
    let templates = mask_templates();
    let mut seen_names = BTreeSet::new();
    for (ln, f) in rows("hcy_template") {
        let idx = f64_of(f[1], ln) as usize;
        let tpl = &templates[idx];
        assert_eq!(tpl.name, f[2], "第 {ln} 行模板名");
        assert_eq!(tpl.description, f[3], "第 {ln} 行说明");
        assert_eq!(tpl.shapes.len(), f64_of(f[4], ln) as usize, "第 {ln} 行形状数");
        seen_names.insert(tpl.name);
    }
    assert_eq!(seen_names.len(), 5, "fixture 应有五个预设");

    let mut n = 0;
    for (ln, f) in rows("hcy_template_point") {
        let idx = f64_of(f[1], ln) as usize;
        let si = f64_of(f[2], ln) as usize;
        let pi = f64_of(f[3], ln) as usize;
        let point = templates[idx].shapes[si][pi];
        assert_bits(point[0], f64_of(f[4], ln), &format!("第 {ln} 行 h"));
        assert_bits(point[1], f64_of(f[5], ln), &format!("第 {ln} 行 c"));
        assert_bits(point[2], f64_of(f[6], ln), &format!("第 {ln} 行 y"));
        n += 1;
    }
    assert_eq!(n, 38, "五个预设一共 38 个点");

    // 模板 → 遮罩：色相过逆畸变（`:1013`），彩度/明度原样
    for (ty, ranges) in [
        (WheelType::Rgb, None),
        (WheelType::Rygb, WheelType::Rygb.ranges()),
        (WheelType::Ryb, WheelType::Ryb.ranges()),
    ] {
        for tpl in &templates {
            let mask = template_mask(tpl, ranges);
            assert_eq!(mask.len(), tpl.shapes.len(), "{ty:?} 的形状数");
            for (shape, floats) in mask.iter().zip(tpl.shapes.iter()) {
                assert_eq!(shape.len(), floats.len());
                for (col, [h, c, y]) in shape.iter().zip(floats.iter()) {
                    let Color::Hcy(col) = col else {
                        panic!("模板颜色应是 HCYColor")
                    };
                    assert_bits(col.h, undistort_hue(*h, ranges), "模板色相");
                    assert_bits(col.c, *c, "模板彩度");
                    assert_bits(col.y, *y, "模板明度");
                }
            }
        }
    }
}

/// 凸包 —— `lib/alg.py:25-69`（真 import）。
#[test]
fn hcy_convex_hull_matches_source() {
    let mut n = 0;
    for (ln, f) in rows("hcy_hull") {
        let n_in = f64_of(f[1], ln) as usize;
        let input = points_of(&f[2..], n_in, ln);
        let out_at = 2 + 2 * n_in;
        let n_out = f64_of(f[out_at], ln) as usize;
        let expected = points_of(&f[out_at + 1..], n_out, ln);
        let actual = convex_hull(&input);
        assert_eq!(actual.len(), expected.len(), "第 {ln} 行凸包点数");
        for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            assert_bits(a.0, e.0, &format!("第 {ln} 行凸包第 {i} 点 x"));
            assert_bits(a.1, e.1, &format!("第 {ln} 行凸包第 {i} 点 y"));
        }
        n += 1;
    }
    assert!(n >= 15, "凸包向量太少：{n}");
}

/// 点在凸多边形内（含边界）—— `lib/alg.py:127-159`。
#[test]
fn hcy_point_in_poly_matches_source() {
    let mut n = 0;
    for (ln, f) in rows("hcy_pip") {
        let np = f64_of(f[1], ln) as usize;
        let poly = points_of(&f[2..], np, ln);
        let x = f64_of(f[2 + 2 * np], ln);
        let y = f64_of(f[3 + 2 * np], ln);
        let expect = bool_of(f[4 + 2 * np], ln);
        assert_eq!(
            point_in_convex_poly((x, y), &poly),
            expect,
            "第 {ln} 行点 ({x}, {y})"
        );
        n += 1;
    }
    assert!(n >= 60, "命中向量太少：{n}");
}

/// 线段垂足（不含端点）—— `lib/alg.py:162-186`。
#[test]
fn hcy_nearest_point_in_segment_matches_source() {
    for (ln, f) in rows("hcy_nps") {
        let a = (f64_of(f[1], ln), f64_of(f[2], ln));
        let b = (f64_of(f[3], ln), f64_of(f[4], ln));
        let p = (f64_of(f[5], ln), f64_of(f[6], ln));
        let has = bool_of(f[7], ln);
        match nearest_point_in_segment(a, b, p) {
            None => assert!(!has, "第 {ln} 行源码给出了垂足"),
            Some(got) => {
                assert!(has, "第 {ln} 行源码返回 None");
                assert_bits(got.0, f64_of(f[8], ln), &format!("第 {ln} 行垂足 x"));
                assert_bits(got.1, f64_of(f[9], ln), &format!("第 {ln} 行垂足 y"));
            }
        }
    }
}

/// 面积与"尺寸" —— `lib/alg.py:84-95` + `hcywheel.py:240-244`。
#[test]
fn hcy_poly_area_and_void_size_match_source() {
    for (ln, f) in rows("hcy_area") {
        let np = f64_of(f[1], ln) as usize;
        let poly = points_of(&f[2..], np, ln);
        assert_bits(
            poly_area(&poly),
            f64_of(f[2 + 2 * np], ln),
            &format!("第 {ln} 行面积"),
        );
    }
    for (ln, f) in rows("hcy_void_size") {
        let np = f64_of(f[1], ln) as usize;
        let poly = points_of(&f[2..], np, ln);
        assert_bits(
            void_size(&poly),
            f64_of(f[2 + 2 * np], ln),
            &format!("第 {ln} 行尺寸"),
        );
    }
}

/// 取色：位置 → HCY（含逆畸变）—— `adjbases.py:1148-1167` +
/// `hcywheel.py:332-336`。
#[test]
fn hcy_wheel_colors_match_source() {
    let mut n = 0;
    let mut types = BTreeSet::new();
    for (ln, f) in rows("hcy_color") {
        let wheel_type = wheel_type_of(f[1], ln);
        types.insert(wheel_type.name());
        let geom = WheelGeometry::for_size(f64_of(f[2], ln), f64_of(f[3], ln), 2.0);
        let x = f64_of(f[4], ln);
        let y = f64_of(f[5], ln);
        let managed = Color::Hcy(HcyColor::new(
            f64_of(f[6], ln),
            f64_of(f[7], ln),
            f64_of(f[8], ln),
        ));
        let expected = HcyColor::new(f64_of(f[9], ln), f64_of(f[10], ln), f64_of(f[11], ln));
        let actual = hcy_wheel_color_at_position(&geom, x, y, &managed, wheel_type.ranges())
            .unwrap_or_else(|| panic!("第 {ln} 行：几何应可用"));
        assert_bits(actual.h, expected.h, &format!("第 {ln} 行 h"));
        assert_bits(actual.c, expected.c, &format!("第 {ln} 行 c"));
        assert_bits(actual.y, expected.y, &format!("第 {ln} 行 y"));
        n += 1;
    }
    assert_eq!(types.len(), 2, "rgb 与 rygb 两种轮都要覆盖");
    assert!(n >= 700, "HCY 取色向量太少：{n}");
}

/// 标记位置：颜色 → 像素 —— `adjbases.py:1301-1315` 套 HCY 钩子。
#[test]
fn hcy_wheel_marker_positions_match_source() {
    let mut n = 0;
    for (ln, f) in rows("hcy_pos") {
        let wheel_type = wheel_type_of(f[1], ln);
        let geom = WheelGeometry::for_size(f64_of(f[2], ln), f64_of(f[3], ln), 2.0);
        let color = Color::Hcy(HcyColor::new(
            f64_of(f[4], ln),
            f64_of(f[5], ln),
            f64_of(f[6], ln),
        ));
        let (x, y) = hcy_wheel_position_for_color(&geom, &color, wheel_type.ranges());
        assert_bits(x, f64_of(f[7], ln), &format!("第 {ln} 行标记 x"));
        assert_bits(y, f64_of(f[8], ln), &format!("第 {ln} 行标记 y"));
        n += 1;
    }
    assert!(n >= 40, "HCY 标记向量太少：{n}");
}

/// 轮上滚轮只改 HCY 明度 —— `hcywheel.py:356-367`。
#[test]
fn hcy_wheel_scroll_matches_source() {
    for (ln, f) in rows("hcy_scroll") {
        let y0 = f64_of(f[1], ln);
        let direction = match f[2] {
            "UP" => ScrollDirection::Up,
            "DOWN" => ScrollDirection::Down,
            "LEFT" => ScrollDirection::Left,
            "RIGHT" => ScrollDirection::Right,
            "SMOOTH" => ScrollDirection::Smooth,
            other => panic!("第 {ln} 行未知方向 {other:?}"),
        };
        let changed = bool_of(f[3], ln);
        let y1 = f64_of(f[4], ln);
        let managed = Color::Hcy(HcyColor::new(0.3, 0.4, y0));
        match hcy_wheel_scroll(&managed, direction) {
            None => assert!(!changed, "第 {ln} 行：源码的 if 不成立时不应落色"),
            Some(Color::Hcy(col)) => {
                assert!(changed, "第 {ln} 行：应落色");
                assert_bits(col.y, y1, &format!("第 {ln} 行明度"));
                assert_bits(col.h, 0.3, "色相不该动");
                assert_bits(col.c, 0.4, "彩度不该动");
            }
            Some(other) => panic!("第 {ln} 行应回写 HCYColor，实得 {}", other.class_name()),
        }
    }
}

/// 编辑器轮子的明度夹取 —— `hcywheel.py:811-822`。
///
/// 两条来源不同的行：`hcy_luma` 是 `get_managed_color()`（读出来的颜色被夹），
/// `hcy_luma_set` 是 `set_managed_color()`（写进去的颜色被夹）。两者共用同一
/// 个 `clamp(col.y, __MIN_LUMA, __MAX_LUMA)`。
#[test]
fn hcy_editor_luma_matches_source() {
    for kind in ["hcy_luma", "hcy_luma_set"] {
        let mut n = 0;
        for (ln, f) in rows(kind) {
            assert_bits(
                editor_luma_y(f64_of(f[1], ln)),
                f64_of(f[2], ln),
                &format!("第 {ln} 行 {kind}"),
            );
            n += 1;
        }
        assert_eq!(n, 9, "{kind} 应有 9 条");
    }
}

/// 明度滑条的采样段数 —— `sliders.py:287-292`。
#[test]
fn hcy_luma_slider_samples_match_source() {
    let mut n = 0;
    for (ln, f) in rows("hcy_samples") {
        let got = hcy_luma_slider_samples(
            f64_of(f[1], ln),
            f64_of(f[2], ln),
            bool_of(f[3], ln),
            f64_of(f[4], ln),
        );
        assert_eq!(got, f64_of(f[5], ln) as i64, "第 {ln} 行段数");
        n += 1;
    }
    assert_eq!(n, 21);
}

/// 遮罩 void = 各颜色位置的凸包（`hcywheel.py:180-204`）。
#[test]
fn hcy_mask_voids_match_source() {
    let cases = mask_cases();
    let mut n = 0;
    for (ln, f) in rows("hcy_mask_voids") {
        let case = cases
            .get(f[1])
            .unwrap_or_else(|| panic!("第 {ln} 行引用了不存在的 case {}", f[1]));
        let geom = WheelGeometry::for_size(case.width, case.height, 2.0);
        let got = mask_voids(&case.mask, &geom, case.wheel_type.ranges());
        let n_voids = f64_of(f[2], ln) as usize;
        assert_eq!(got.len(), n_voids, "第 {ln} 行 void 数");
        let mut cursor = 3;
        for (vi, void) in got.iter().enumerate() {
            let np = f64_of(f[cursor], ln) as usize;
            cursor += 1;
            let expected = points_of(&f[cursor..], np, ln);
            cursor += 2 * np;
            assert_eq!(void.len(), np, "第 {ln} 行第 {vi} 个 void 点数");
            for (i, (a, e)) in void.iter().zip(expected.iter()).enumerate() {
                assert_bits(a.0, e.0, &format!("第 {ln} 行 void {vi} 点 {i} x"));
                assert_bits(a.1, e.1, &format!("第 {ln} 行 void {vi} 点 {i} y"));
            }
        }
        assert_eq!(cursor, f.len(), "第 {ln} 行有剩余字段没读");
        n += 1;
    }
    assert_eq!(n, 14);
}

/// 遮罩绘制时的两层过滤（点数 < 3、尺寸 < `radius * MIN_SHAPE_SIZE`）——
/// `hcywheel.py:288-295`。ground truth 来自**真源码** `draw_mask()` 里
/// 实际 `move_to/line_to` 的点（脚本用记录式 cairo 替身接住）。
#[test]
fn hcy_mask_draw_filter_matches_source() {
    let cases = mask_cases();
    let mut saw_empty = false;
    for (ln, f) in rows("hcy_mask_draw") {
        let case = cases
            .get(f[1])
            .unwrap_or_else(|| panic!("第 {ln} 行引用了不存在的 case {}", f[1]));
        let geom = WheelGeometry::for_size(case.width, case.height, 2.0);
        let got = visible_mask_voids(&case.mask, &geom, case.wheel_type.ranges(), geom.radius);
        let n_drawn = f64_of(f[2], ln) as usize;
        assert_eq!(got.len(), n_drawn, "第 {ln} 行画出的 void 数");
        if n_drawn == 0 {
            saw_empty = true;
        }
        let mut cursor = 3;
        for (vi, void) in got.iter().enumerate() {
            let np = f64_of(f[cursor], ln) as usize;
            cursor += 1;
            let expected = points_of(&f[cursor..], np, ln);
            cursor += 2 * np;
            for (i, (a, e)) in void.iter().zip(expected.iter()).enumerate() {
                assert_bits(a.0, e.0, &format!("第 {ln} 行 void {vi} 点 {i} x"));
                assert_bits(a.1, e.1, &format!("第 {ln} 行 void {vi} 点 {i} y"));
            }
        }
        assert_eq!(cursor, f.len(), "第 {ln} 行有剩余字段没读");
    }
    assert!(saw_empty, "fixture 里应有被尺寸过滤掉的用例");
}

/// 遮罩取色裁剪 —— `hcywheel.py:206-238`。
#[test]
fn hcy_masked_sampling_matches_source() {
    let cases = mask_cases();
    let mut n = 0;
    for (ln, f) in rows("hcy_masked") {
        let case = cases
            .get(f[1])
            .unwrap_or_else(|| panic!("第 {ln} 行引用了不存在的 case {}", f[1]));
        let geom = WheelGeometry::for_size(case.width, case.height, 2.0);
        let x = f64_of(f[2], ln);
        let y = f64_of(f[3], ln);
        // 脚本里 managed 固定是 HCYColor(0, 0, 0.5)
        let managed = Color::Hcy(HcyColor::new(0.0, 0.0, 0.5));
        let got = masked_color_at_position(&case.mask, &geom, x, y, &managed, case.wheel_type.ranges())
            .unwrap_or_else(|| panic!("第 {ln} 行：几何应可用"));
        assert_bits(got.h, f64_of(f[4], ln), &format!("第 {ln} 行 h"));
        assert_bits(got.c, f64_of(f[5], ln), &format!("第 {ln} 行 c"));
        assert_bits(got.y, f64_of(f[6], ln), &format!("第 {ln} 行 y"));
        n += 1;
    }
    assert!(n >= 300, "遮罩取色向量太少：{n}");
}

/// `colors_to_mask_void` 与 `mask_voids` 的组合等于"逐形状的凸包"，
/// 且点数 < 3 的形状不参与（`hcywheel.py:186` / `:200`）。
#[test]
fn hcy_mask_void_helpers_agree() {
    let cases = mask_cases();
    let (tag, case) = cases.iter().next().expect("至少一个用例");
    let geom = WheelGeometry::for_size(case.width, case.height, 2.0);
    let voids = mask_voids(&case.mask, &geom, case.wheel_type.ranges());
    let expected: Vec<Vec<Point>> = case
        .mask
        .iter()
        .filter(|s| s.len() >= 3)
        .map(|s| colors_to_mask_void(s, &geom, case.wheel_type.ranges()))
        .collect();
    assert_eq!(voids, expected, "{tag}");
}

