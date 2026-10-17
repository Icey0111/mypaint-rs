//! `lib/color.py` 的 doctest 逐条移植 —— 源码里写死的期望值。
//!
//! `lib/color.py` 自带一批 doctest（`python -m lib.color` 就跑它们，`:919-930`），
//! 里面是**源码作者写死的期望值**：相等语义的边界、插值出来的十六进制串、
//! `to_fill_pixel` 的位模式等。这里把它们搬成 Rust 测试，行号标在每条上。
//!
//! 与 `tests/color_vectors.rs` 的分工：那边是 2.3 万条"真源码跑出来的向量"
//! （逐位），这边是源码**文档里承诺过**的那些具体例子 —— 后者能抓住
//! "向量对了但语义写反了"这类问题（比如相等性豁免条件的方向）。

use pigment_engine::color::{
    colors_match, Color, HcyColor, HsvColor, RgbColor, UIColor, YcbcrColor, CONTRASTING_K,
};

/// `rgb.get_luma()`：`lib/color.py:114-128` 的示例（`0.6` 的灰）。
#[test]
fn luma_docstring() {
    let l = RgbColor::new(0.6, 0.6, 0.6).get_luma();
    assert!((l - 0.6).abs() < 1e-12, "0.6 的灰 luma 应为 0.6，实得 {l}");
}

/// `to_greyscale()`：`lib/color.py:130-143` —— 结果三通道必须相等。
#[test]
fn to_greyscale_docstring() {
    let g = RgbColor::new(1.0, 0.8, 0.2).to_greyscale();
    assert_eq!(g.r, g.g);
    assert_eq!(g.g, g.b);
}

/// `to_contrasting()`：`lib/color.py:145-155` —— 与原件不同。
#[test]
fn to_contrasting_docstring() {
    let col = Color::Rgb(RgbColor::new(1.0, 0.8, 0.2));
    let other = col.to_contrasting(CONTRASTING_K);
    assert!(!colors_match(&col, &Color::Rgb(other)), "反差色不该等于原色");
}

/// `to_fill_pixel()`：`lib/color.py:210-220` —— 白色是 `0xffffffff`。
#[test]
fn to_fill_pixel_docstring() {
    let px = RgbColor::new(1.0, 1.0, 1.0).to_fill_pixel();
    assert_eq!(format!("{px:08x}"), "ffffffff");
}

/// `RGBColor.interpolate()`：`lib/color.py:309-327` 的两条示例。
#[test]
fn rgb_interpolate_docstring() {
    let white = RgbColor::new(1.0, 1.0, 1.0);
    let black = RgbColor::new(0.0, 0.0, 0.0);
    let fwd: Vec<String> = white
        .interpolate_rgb(&black, 3)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(fwd, ["#ffffff", "#7f7f7f", "#000000"]);
    let back: Vec<String> = black
        .interpolate_rgb(&white, 3)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(back, ["#000000", "#7f7f7f", "#ffffff"]);
}

/// `HSVColor.interpolate()`：`lib/color.py:408-444` —— 色相走最短弧，
/// 所以红↔绿的中点必须是纯黄（RGB 插值会得到暗黄，见 `:418-422`）。
#[test]
fn hsv_interpolate_docstring() {
    let red = HsvColor::new(0.0, 1.0, 1.0);
    let green = HsvColor::new(1.0 / 3.0, 1.0, 1.0);
    let g2r: Vec<String> = green
        .interpolate_hsv(&red, 3)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(g2r, ["#00ff00", "#ffff00", "#ff0000"]);
    let r2g: Vec<String> = red
        .interpolate_hsv(&green, 3)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(r2g, ["#ff0000", "#ffff00", "#00ff00"]);
    // 源码 `:420-422`：同两点在 RGB 空间插值是暗淡的 `#7f7f00`
    let red_rgb = RgbColor::from_color(&Color::Hsv(red));
    let rgb_mid = red_rgb
        .interpolate(&Color::Hsv(green), 3)
        .into_iter()
        .map(|c| c.to_hex_str())
        .collect::<Vec<_>>();
    assert_eq!(rgb_mid, ["#ff0000", "#7f7f00", "#00ff00"]);
}

/// `HCYColor.interpolate()`：`lib/color.py:562-599` —— 同彩度插值保持彩度，
/// 且六条十六进制串是源码写死的期望。
#[test]
fn hcy_interpolate_docstring() {
    let red = HcyColor::new(0.0, 0.8, 0.5);
    let green = HcyColor::new(1.0 / 3.0, 0.8, 0.5);
    let g2r: Vec<String> = green
        .interpolate_hcy(&red, 5)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(g2r, ["#19c619", "#5ea319", "#8c8c19", "#c46f19", "#e55353"]);
    let r2g: Vec<String> = red
        .interpolate_hcy(&green, 5)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(r2g, ["#e55353", "#c46f19", "#8c8c19", "#5ea319", "#19c619"]);
    // 源码 `:577-578`：夹在中间的五点彩度都是 0.8
    for c in red.interpolate_hcy(&green, 5) {
        assert!((c.c - 0.8).abs() < 1e-12, "中间彩度应恒为 0.8，实得 {}", c.c);
    }
}

/// `YCbCrColor.interpolate()`：`lib/color.py:690-716` —— 与 RGB 插值只差 1/255
/// 的舍入（源码注释 `:698-700`）。
#[test]
fn ycbcr_interpolate_docstring() {
    let yellow = YcbcrColor::from_color(&Color::Rgb(RgbColor::new(1.0, 1.0, 0.0)));
    let red = YcbcrColor::from_color(&Color::Rgb(RgbColor::new(1.0, 0.0, 0.0)));
    let hexes: Vec<String> = yellow
        .interpolate_ycbcr(&red, 3)
        .iter()
        .map(|c| c.to_hex_str())
        .collect();
    assert_eq!(hexes, ["#feff00", "#ff7f00", "#ff0000"]);
}

/// `__eq__` 的边界：`lib/color.py:446-481`（HSV）与 `:601-638`（HCY）。
#[test]
fn equality_docstrings() {
    // HSV：明度 0 时不同色相/饱和度必须相等；明度 1 时**不**豁免
    assert!(colors_match(
        &Color::Hsv(HsvColor::new(0.7, 0.45, 0.0)),
        &Color::Hsv(HsvColor::new(0.4, 0.55, 0.0))
    ));
    assert!(!colors_match(
        &Color::Hsv(HsvColor::new(0.7, 0.45, 1.0)),
        &Color::Hsv(HsvColor::new(0.4, 0.55, 1.0))
    ));
    // HCY：明度 0 **和** 1 都豁免
    assert!(colors_match(
        &Color::Hcy(HcyColor::new(0.7, 0.45, 0.0)),
        &Color::Hcy(HcyColor::new(0.4, 0.55, 0.0))
    ));
    assert!(colors_match(
        &Color::Hcy(HcyColor::new(0.7, 0.45, 1.0)),
        &Color::Hcy(HcyColor::new(0.4, 0.55, 1.0))
    ));
    // 跨类：`c2 == HCYColor(color=c2)` 必须成立（RGB 在左，走 round(3)）
    let c2 = RgbColor::new(0.4, 0.55, 0.45);
    let c2hcy = HcyColor::from_color(&Color::Rgb(c2));
    assert!(colors_match(&Color::Rgb(c2), &Color::Hcy(c2hcy)));
    // 不同颜色不相等
    assert!(!colors_match(
        &Color::Rgb(RgbColor::new(0.7, 0.45, 0.55)),
        &Color::Rgb(RgbColor::new(0.4, 0.55, 0.45))
    ));
}

/// 构造函数的 docstring：`lib/color.py:277-286`、`:373-382`、`:509-519`。
#[test]
fn constructors_docstrings() {
    // RGBColor(rgb=…) 与逐分量一致
    assert_eq!(
        RgbColor::new(1.0, 0.0, 1.0),
        RgbColor::from_rgb([1.0, 0.0, 1.0])
    );
    // `RGBColor(color=HSVColor(0.0, 0.0, 0.5))` → 中灰
    let grey = RgbColor::from_color(&Color::Hsv(HsvColor::new(0.0, 0.0, 0.5)));
    assert_eq!(grey.repr(), "<RGBColor r=0.5000, g=0.5000, b=0.5000>");
    // `HSVColor(color=RGBColor(0.5,0.5,0.5))` → h=0, s=0, v=0.5
    let hsv = HsvColor::from_color(&Color::Rgb(RgbColor::new(0.5, 0.5, 0.5)));
    assert_eq!(hsv.repr(), "<HSVColor h=0.0000, s=0.0000, v=0.5000>");
    // `HCYColor(color=RGBColor(0.5,0.5,0.5))` → c=0, y=0.5
    let hcy = HcyColor::from_color(&Color::Rgb(RgbColor::new(0.5, 0.5, 0.5)));
    assert_eq!(hcy.repr(), "<HCYColor h=0.0000, c=0.0000, y=0.5000>");
}

/// `new_from_hex_str` 的 docstring 与源码语义：`lib/color.py:193-203`。
#[test]
fn hex_parse_docstring() {
    let red = RgbColor::new_from_hex_str("#ff0000");
    assert_eq!((red.r, red.g, red.b), (1.0, 0.0, 0.0));
    // 3 位简写（除数是 0xF，源码 `:190`）
    assert_eq!(
        RgbColor::new_from_hex_str("#f00"),
        RgbColor::new_from_hex_str("#ff0000")
    );
    // 非法输入不报错，退回默认灰 `[0.5, 0.5, 0.5]`
    assert_eq!(RgbColor::new_from_hex_str("nope"), RgbColor::new(0.5, 0.5, 0.5));
    // `0x` 前缀（小写）也算
    assert_eq!(
        RgbColor::new_from_hex_str("0xff0000"),
        RgbColor::new_from_hex_str("#ff0000")
    );
}

/// `to_hex_str(prefix=...)`：前缀可换（`lib/color.py:205-208`）。
#[test]
fn to_hex_str_prefix_docstring() {
    let c = RgbColor::new(1.0, 0.5, 0.0);
    assert_eq!(c.to_hex_str(), "#ff7f00");
    assert_eq!(c.to_hex_str_with("0x"), "0xff7f00");
    assert_eq!(c.to_hex_str_with(""), "ff7f00");
}

/// `HCYColor.get_hsv()` 的纯黑特例（`lib/color.py:541-547`）：
/// `c != 0` 时即使 y=0 也要把饱和度报出来，否则"走向黑"会丢色相语义。
#[test]
fn hcy_black_special_case_docstring() {
    let black = HcyColor::new(0.25, 0.5, 0.0);
    let hsv = black.get_hsv();
    assert_eq!(hsv[0], 0.25, "色相直接用存下的 h");
    assert_eq!(hsv[2], 0.0, "明度仍是 0");
    assert!(hsv[1] > 0.0, "彩度不为 0 时饱和度必须补出来，实得 {}", hsv[1]);
    // 而 c == 0 的纯黑：饱和度自然就是 0
    let pure_black = HcyColor::new(0.25, 0.0, 0.0);
    assert_eq!(pure_black.get_hsv()[1], 0.0);
}

/// `HCYColor(color=…)。get_hsv()` 的往返（`lib/color.py:521-531` 的
/// "纯黑但饱和不为 0" 分支）：HSV 里 s≠0、v=0 时，经 HCY 转一圈 s 仍非零。
#[test]
fn hcy_from_hsv_keeps_saturation_at_black_docstring() {
    let hsv = HsvColor::new(0.4, 0.7, 0.0);
    let hcy = HcyColor::from_color(&Color::Hsv(hsv));
    assert!(hcy.c > 0.0, "纯黑但饱和度不为 0 时彩度要保住，实得 {}", hcy.c);
    let back = hcy.get_hsv();
    assert_eq!(back[0], 0.4);
    assert!(back[1] > 0.0, "饱和度也要保住，实得 {}", back[1]);
}
