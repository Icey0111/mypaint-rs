//! 颜色向量逐位门禁 —— 拿真 `lib/color.py` 算出的 fixture 与引擎逐点比对。
//!
//! fixture：`tests/fixtures/color_vectors.txt`（23795 条向量），由
//! `tests/tools/gen-color-vectors.py` 生成（`.workbuddy\tools\`
//! 下那份是同一条命令的转发入口 —— `.workbuddy` 不在本仓库内）：
//!
//! ```text
//! python crates/pigment-engine/tests/tools/gen-color-vectors.py
//! ```
//!
//! 脚本 `import` 的是**真** `lib/color.py`（不是重写的公式），浮点用 Python
//! `repr()`（最短往返）写出；Rust 的 `str::parse::<f64>()` 是正确舍入的，所以
//! 解析出来的位模式与 Python 侧完全相同 —— 这里因此可以逐位 `==`，
//! **没有任何容差**。
//!
//! 逐位比较用 `to_bits()`，唯一的例外是 `+0.0` 与 `-0.0` 视为相同：
//! Python 的 `%` 在结果为零时返回 `+0.0`，Rust 的 `rem_euclid` 可能保留 `-0.0`，
//! 两者在后续运算里完全等价（`x*6.0`、`int()`、乘法都不区分零的符号），
//! 所以不把这条当作不一致 —— 见 `same_bits()`。

use std::collections::BTreeMap;
use std::sync::OnceLock;

use pigment_engine::color::{
    colors_match, hcy_to_rgb, hsv_to_rgb, rgb_to_hcy, rgb_to_hsv, rgb_to_ycbcr_bt601,
    ycbcr_to_rgb_bt601, BrushStep, Color, ColorAction, HcyColor, HsvColor, RgbColor, UIColor,
    YcbcrColor, CONTRASTING_K,
};

/// fixture 的绝对路径（编译期由 `CARGO_MANIFEST_DIR` 定位）。
fn fixture_path() -> String {
    format!(
        "{}/tests/fixtures/color_vectors.txt",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// 整个 fixture 只读一次（2 MB 文本，逐个测试重复读没必要）。
fn fixture() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        std::fs::read_to_string(fixture_path()).unwrap_or_else(|e| {
            panic!(
                "读不到 fixture {}：{e}（先跑 .workbuddy/tools/gen-color-vectors.py）",
                fixture_path()
            )
        })
    })
}

/// fixture 的数据行；断言条数用。
fn data_lines() -> impl Iterator<Item = (usize, &'static str)> {
    fixture()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
}

/// 一行的 tab 分隔字段。
fn fields(line: &str) -> Vec<&str> {
    line.split('\t').collect()
}

/// 解析一个浮点字段。
fn parse_f64(tok: &str, what: &str, ln: usize) -> f64 {
    tok.parse::<f64>()
        .unwrap_or_else(|e| panic!("第 {ln} 行 {what} 不是浮点：{tok:?}（{e}）"))
}

/// 解析三个浮点字段。
fn triple(toks: &[&str], ln: usize) -> [f64; 3] {
    assert_eq!(toks.len(), 3, "第 {ln} 行应有 3 个通道：{toks:?}");
    [
        parse_f64(toks[0], "通道", ln),
        parse_f64(toks[1], "通道", ln),
        parse_f64(toks[2], "通道", ln),
    ]
}

/// 逐位相等（`+0.0` 与 `-0.0` 视为相等，理由见模块文档）。
fn same_bits(actual: f64, expected: f64) -> bool {
    actual.to_bits() == expected.to_bits() || (actual == 0.0 && expected == 0.0)
}

/// 断言一个三元组逐位相等。
fn assert_triple(actual: [f64; 3], expected: [f64; 3], ctx: &str) {
    for i in 0..3 {
        assert!(
            same_bits(actual[i], expected[i]),
            "{ctx} 分量 {i} 不一致：实际 {:?}（bits {:#x}）期望 {:?}（bits {:#x}）",
            actual[i],
            actual[i].to_bits(),
            expected[i],
            expected[i].to_bits(),
        );
    }
}

/// 按 fixture 里的 kind 串造颜色。
fn color_of(kind: &str, v: [f64; 3]) -> Color {
    match kind {
        "rgb" => Color::Rgb(RgbColor::from_rgb(v)),
        "hsv" => Color::Hsv(HsvColor::from_hsv(v)),
        "hcy" => Color::Hcy(HcyColor::from_hcy(v)),
        "ycbcr" => Color::Ycbcr(YcbcrColor::from_ycbcr(v)),
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// 按 kind 取该色彩空间的三个代表分量（`interp` 用例用）。
fn space_triple(kind: &str, c: Color) -> [f64; 3] {
    match kind {
        "rgb" => c.get_rgb(),
        "hsv" => c.get_hsv(),
        "hcy" => {
            let Color::Hcy(h) = c else {
                panic!("interp 期望 HCYColor，实际 {}", c.class_name())
            };
            [h.h, h.c, h.y]
        }
        "ycbcr" => {
            let Color::Ycbcr(y) = c else {
                panic!("interp 期望 YCbCrColor，实际 {}", c.class_name())
            };
            [y.y, y.cb, y.cr]
        }
        other => panic!("未知颜色类型 {other:?}"),
    }
}

/// 每类向量各跑一遍的公共骨架：扫描全部行、只处理 `kind` 那一类。
fn for_each(kind: &str, mut check: impl FnMut(usize, &[&str])) {
    for (ln, line) in data_lines() {
        let toks = fields(line);
        if toks[0] == kind {
            check(ln, &toks);
        }
    }
}

// ---------------------------------------------------------------------------
// 换算：正向
// ---------------------------------------------------------------------------

#[test]
fn rgb_to_hsv_matches_source() {
    let mut n = 0;
    for_each("rgb2hsv", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            rgb_to_hsv(rgb),
            expected,
            &format!("第 {ln} 行 rgb2hsv {rgb:?}"),
        );
        n += 1;
    });
    // RGB 立方体网格 16³ = 4096 点，外加边界点；少于 4096 说明 fixture 被截断
    assert!(n >= 4096, "rgb2hsv 向量只有 {n} 条，网格应至少 4096 条");
}

#[test]
fn rgb_to_hcy_matches_source() {
    let mut n = 0;
    for_each("rgb2hcy", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            rgb_to_hcy(rgb),
            expected,
            &format!("第 {ln} 行 rgb2hcy {rgb:?}"),
        );
        n += 1;
    });
    assert!(n >= 4096, "rgb2hcy 向量只有 {n} 条");
}

#[test]
fn rgb_to_ycbcr_matches_source() {
    let mut n = 0;
    for_each("rgb2ycbcr", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            rgb_to_ycbcr_bt601(rgb),
            expected,
            &format!("第 {ln} 行 rgb2ycbcr {rgb:?}"),
        );
        n += 1;
    });
    assert!(n >= 4096, "rgb2ycbcr 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 换算：反向
// ---------------------------------------------------------------------------

#[test]
fn hsv_to_rgb_matches_source() {
    let mut n = 0;
    for_each("hsv2rgb", |ln, t| {
        let hsv = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            hsv_to_rgb(hsv),
            expected,
            &format!("第 {ln} 行 hsv2rgb {hsv:?}"),
        );
        n += 1;
    });
    assert!(n > 400, "hsv2rgb 向量只有 {n} 条");
}

#[test]
fn hcy_to_rgb_matches_source() {
    let mut n = 0;
    for_each("hcy2rgb", |ln, t| {
        let hcy = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            hcy_to_rgb(hcy),
            expected,
            &format!("第 {ln} 行 hcy2rgb {hcy:?}"),
        );
        n += 1;
    });
    assert!(n > 400, "hcy2rgb 向量只有 {n} 条");
}

#[test]
fn ycbcr_to_rgb_matches_source() {
    let mut n = 0;
    for_each("ycbcr2rgb", |ln, t| {
        let ycc = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        assert_triple(
            ycbcr_to_rgb_bt601(ycc),
            expected,
            &format!("第 {ln} 行 ycbcr2rgb {ycc:?}"),
        );
        n += 1;
    });
    assert!(n > 300, "ycbcr2rgb 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 跨模型（HCYColor 构造路径与 get_hsv 覆写）
// ---------------------------------------------------------------------------

#[test]
fn hcy_from_hsv_matches_source() {
    let mut n = 0;
    for_each("hsv2hcy", |ln, t| {
        let hsv = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        let c = HcyColor::from_color(&Color::Hsv(HsvColor::from_hsv(hsv)));
        assert_triple(
            [c.h, c.c, c.y],
            expected,
            &format!("第 {ln} 行 hsv2hcy {hsv:?}"),
        );
        n += 1;
    });
    assert!(n > 400, "hsv2hcy 向量只有 {n} 条");
}

#[test]
fn hcy_get_hsv_matches_source() {
    let mut n = 0;
    for_each("hcy2hsv", |ln, t| {
        let hcy = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        let c = HcyColor::from_hcy(hcy);
        assert_triple(
            c.get_hsv(),
            expected,
            &format!("第 {ln} 行 hcy2hsv {hcy:?}"),
        );
        n += 1;
    });
    assert!(n > 400, "hcy2hsv 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 往返（源码里往返不是恒等变换，所以结果单独固化）
// ---------------------------------------------------------------------------

#[test]
fn hsv_round_trip_matches_source() {
    let mut n = 0;
    for_each("hsv_rt", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        let actual = hsv_to_rgb(rgb_to_hsv(rgb));
        assert_triple(actual, expected, &format!("第 {ln} 行 hsv_rt {rgb:?}"));
        n += 1;
    });
    assert!(n > 800, "hsv_rt 向量只有 {n} 条");
}

#[test]
fn hcy_round_trip_matches_source() {
    let mut n = 0;
    for_each("hcy_rt", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        let actual = hcy_to_rgb(rgb_to_hcy(rgb));
        assert_triple(actual, expected, &format!("第 {ln} 行 hcy_rt {rgb:?}"));
        n += 1;
    });
    assert!(n > 800, "hcy_rt 向量只有 {n} 条");
}

#[test]
fn ycbcr_round_trip_matches_source() {
    let mut n = 0;
    for_each("ycbcr_rt", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = triple(&t[4..7], ln);
        let actual = ycbcr_to_rgb_bt601(rgb_to_ycbcr_bt601(rgb));
        assert_triple(actual, expected, &format!("第 {ln} 行 ycbcr_rt {rgb:?}"));
        n += 1;
    });
    assert!(n > 800, "ycbcr_rt 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// luma（基类与两个覆写实现）
// ---------------------------------------------------------------------------

#[test]
fn luma_matches_source() {
    let mut n = 0;
    for_each("luma", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = parse_f64(t[4], "luma", ln);
        let actual = RgbColor::from_rgb(rgb).get_luma();
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 luma {rgb:?}：实际 {actual:?} 期望 {expected:?}"
        );
        n += 1;
    });
    assert!(n > 800, "luma 向量只有 {n} 条");
}

#[test]
fn hcy_luma_matches_source() {
    let mut n = 0;
    for_each("hcy_luma", |ln, t| {
        let hcy = triple(&t[1..4], ln);
        let expected = parse_f64(t[4], "luma", ln);
        let actual = HcyColor::from_hcy(hcy).get_luma();
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 hcy_luma {hcy:?}：实际 {actual:?} 期望 {expected:?}"
        );
        n += 1;
    });
    assert!(n > 400, "hcy_luma 向量只有 {n} 条");
}

#[test]
fn ycbcr_luma_matches_source() {
    let mut n = 0;
    for_each("ycc_luma", |ln, t| {
        let ycc = triple(&t[1..4], ln);
        let expected = parse_f64(t[4], "luma", ln);
        let actual = YcbcrColor::from_ycbcr(ycc).get_luma();
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 ycc_luma {ycc:?}：实际 {actual:?} 期望 {expected:?}"
        );
        n += 1;
    });
    assert!(n > 300, "ycc_luma 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 对比色 / hex / fill pixel
// ---------------------------------------------------------------------------

#[test]
fn contrasting_matches_source() {
    for_each("contrasting", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = parse_f64(t[4], "contrasting", ln);
        let actual = RgbColor::from_rgb(rgb).to_contrasting(CONTRASTING_K).r();
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 contrasting {rgb:?}：实际 {actual:?} 期望 {expected:?}"
        );
    });
}

#[test]
fn to_hex_str_matches_source() {
    for_each("hex", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected = t[4];
        let actual = RgbColor::from_rgb(rgb).to_hex_str();
        assert_eq!(actual, expected, "第 {ln} 行 hex {rgb:?}");
    });
}

#[test]
fn from_hex_str_matches_source() {
    for_each("from_hex", |ln, t| {
        let s = t[1];
        let expected = triple(&t[2..5], ln);
        let actual = RgbColor::new_from_hex_str(s);
        assert_triple(
            [actual.r, actual.g, actual.b],
            expected,
            &format!("第 {ln} 行 from_hex {s:?}"),
        );
    });
}

#[test]
fn to_fill_pixel_matches_source() {
    for_each("fillpixel", |ln, t| {
        let rgb = triple(&t[1..4], ln);
        let expected: u32 = t[4]
            .parse()
            .unwrap_or_else(|e| panic!("第 {ln} 行 fillpixel 不是整数：{:?}（{e}）", t[4]));
        let actual = RgbColor::from_rgb(rgb).to_fill_pixel();
        assert_eq!(actual, expected, "第 {ln} 行 fillpixel {rgb:?}");
    });
}

// ---------------------------------------------------------------------------
// 相等语义（非对称，两个方向都断言）
// ---------------------------------------------------------------------------

#[test]
fn eq_semantics_match_source() {
    let mut n = 0;
    for_each("eq", |ln, t| {
        // eq kindA a0 a1 a2 kindB b0 b1 b2 ab ba
        let ka = t[1];
        let a = color_of(ka, triple(&t[2..5], ln));
        let kb = t[5];
        let b = color_of(kb, triple(&t[6..9], ln));
        let ab = t[9] == "1";
        let ba = t[10] == "1";
        assert_eq!(
            a.eq_src(&b),
            ab,
            "第 {ln} 行 eq：{} == {} 期望 {ab}",
            a.repr(),
            b.repr()
        );
        assert_eq!(
            b.eq_src(&a),
            ba,
            "第 {ln} 行 eq：{} == {} 期望 {ba}",
            b.repr(),
            a.repr()
        );
        assert_eq!(
            colors_match(&a, &b),
            ab || ba,
            "第 {ln} 行 eq：colors_match 应等于两个方向的或"
        );
        n += 1;
    });
    assert!(n >= 20, "eq 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 调整动作（`gui/document.py` 的回调原文）
// ---------------------------------------------------------------------------

#[test]
fn color_adjustments_match_source() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for_each("adjust", |ln, t| {
        // adjust action h s v h' s' v'
        let act = ColorAction::from_action_name(t[1])
            .unwrap_or_else(|| panic!("第 {ln} 行未知颜色动作 {:?}", t[1]));
        let hsv = triple(&t[2..5], ln);
        let expected = triple(&t[5..8], ln);
        assert_triple(
            act.apply(hsv),
            expected,
            &format!("第 {ln} 行 adjust {} {}", t[1], hsv.len()),
        );
        *counts.entry(act.action_name()).or_default() += 1;
    });
    for act in ColorAction::ALL {
        let c = counts.get(act.action_name()).copied().unwrap_or(0);
        assert!(c > 100, "{} 只有 {c} 条 adjust 向量", act.action_name());
    }
}

#[test]
fn brush_steps_match_source() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for_each("brushstep", |ln, t| {
        // brushstep action value value'
        let step = BrushStep::from_action_name(t[1])
            .unwrap_or_else(|| panic!("第 {ln} 行未知笔刷动作 {:?}", t[1]));
        let value = parse_f64(t[2], "value", ln);
        let expected = parse_f64(t[3], "value'", ln);
        let actual = step.apply(value);
        assert!(
            same_bits(actual, expected),
            "第 {ln} 行 brushstep {} {value:?}：实际 {actual:?} 期望 {expected:?}",
            t[1]
        );
        *counts.entry(step.action_name()).or_default() += 1;
    });
    for step in BrushStep::ALL {
        let c = counts.get(step.action_name()).copied().unwrap_or(0);
        assert!(c > 5, "{} 只有 {c} 条 brushstep 向量", step.action_name());
    }
}

// ---------------------------------------------------------------------------
// 插值（四个子类各自一份实现）
// ---------------------------------------------------------------------------

#[test]
fn interpolate_matches_source() {
    let mut n = 0;
    for_each("interp", |ln, t| {
        // interp kind a0 a1 a2 b0 b1 b2 steps flat...
        let kind = t[1];
        let a = color_of(kind, triple(&t[2..5], ln));
        let b = color_of(kind, triple(&t[5..8], ln));
        let steps: usize = t[8]
            .parse()
            .unwrap_or_else(|e| panic!("第 {ln} 行 steps 不是整数：{:?}（{e}）", t[8]));
        let flat = &t[9..];
        assert_eq!(
            flat.len(),
            steps * 3,
            "第 {ln} 行 interp 展平长度应为 {}",
            steps * 3
        );
        let got = a.interpolate(&b, steps);
        assert_eq!(got.len(), steps, "第 {ln} 行 interp 元素个数");
        for (i, c) in got.into_iter().enumerate() {
            let expected = [
                parse_f64(flat[i * 3], "分量", ln),
                parse_f64(flat[i * 3 + 1], "分量", ln),
                parse_f64(flat[i * 3 + 2], "分量", ln),
            ];
            assert_triple(
                space_triple(kind, c),
                expected,
                &format!("第 {ln} 行 interp {kind} 第 {i} 个"),
            );
        }
        n += 1;
    });
    assert!(n >= 9, "interp 向量只有 {n} 条");
}

// ---------------------------------------------------------------------------
// 自检：fixture 覆盖面
// ---------------------------------------------------------------------------

#[test]
fn fixture_covers_all_vector_kinds() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, line) in data_lines() {
        let toks = fields(line);
        *counts.entry(toks[0]).or_default() += 1;
    }
    let expected = [
        "rgb2hsv",
        "hsv2rgb",
        "rgb2hcy",
        "hcy2rgb",
        "rgb2ycbcr",
        "ycbcr2rgb",
        "hsv2hcy",
        "hcy2hsv",
        "hsv_rt",
        "hcy_rt",
        "ycbcr_rt",
        "luma",
        "hcy_luma",
        "ycc_luma",
        "contrasting",
        "hex",
        "from_hex",
        "fillpixel",
        "eq",
        "adjust",
        "brushstep",
        "interp",
    ];
    for kind in expected {
        assert!(
            counts.get(kind).copied().unwrap_or(0) > 0,
            "fixture 缺少 {kind} 向量（现有：{counts:?}）"
        );
    }
    let total: usize = counts.values().sum();
    assert_eq!(
        total, 23795,
        "fixture 向量总数变了（{total}），核对生成器输出"
    );
}
