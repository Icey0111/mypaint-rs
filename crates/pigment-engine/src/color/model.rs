//! 颜色对象 —— `lib/color.py` 的 `UIColor` 类层次（`UIColor` / `RGBColor` /
//! `HSVColor` / `HCYColor` / `YCbCrColor`）。
//!
//! ## 为什么是"一个 trait + 四个结构体 + 一个枚举"
//!
//! 源码是"轻量多态结构"（`lib/color.py:13-18`）：四个子类都能互相构造、都能
//! 给出 RGB/HSV 三元组，共享的 `UIColor` 基类提供 `get_rgb()`（抽象）与
//! `get_hsv()`（默认用 `colorsys.rgb_to_hsv`，子类可覆写）等。Rust 里对应：
//!
//! - [`UIColor`]：基类接口（默认方法就是源码基类的实现）；
//! - [`RgbColor`] / [`HsvColor`] / [`HcyColor`] / [`YcbcrColor`]：四个子类；
//! - [`Color`]：**带类型标签**的持有者 —— 源码里"中央共享颜色的类型会随调整器
//!   所在色彩空间改变"（`lib/color.py:13-18`）靠的就是持有 `UIColor` 实例本身，
//!   `__eq__` 的语义也**按左操作数的类**分派（`HSVColor.__eq__` 与
//!   `HCYColor.__eq__` 不同），所以类型标签必须留到运行期。
//!
//! ## 相等语义是非对称的（照抄源码）
//!
//! 四个子类的 `__eq__` 各写各的（`lib/color.py:329`/`:446`/`:601`/`:718`），
//! 而且 `HCYColor.__eq__` 遇到非 HCY 对象时会因 `other.c` 抛 `AttributeError`
//! 而**退回** `UIColor.__eq__` 的 8bpc 比较。所以 [`Color`] **没有**实现
//! `PartialEq`（那会承诺对称性），而是提供 [`UIColor::eq_src`]（= Python 的
//! `self == other`，带方向）与 [`colors_match`]（= `push_history()` 里那句
//! `c == color or color == c`，`gui/colors/adjbases.py:249-255`）。
//!
//! `lib/color.py` 里依赖 GTK 的那一个方法（`new_from_pixbuf_average()`，
//! `:222-262`，吃 `GdkPixbuf`）不在本模块：Rust 侧没有 pixbuf 类型，
//! 需要平均色的地方（背景色）由调用方自己算 —— 记账在 B4-1 的交付说明里。

use super::space::{
    hcy_to_rgb, hsv_to_rgb, rgb_to_hcy, rgb_to_hsv, rgb_to_ycbcr_bt601, ycbcr_to_rgb_bt601,
};

/// `UIColor.new_from_hex_str()` 的默认返回值（`lib/color.py:194` 的
/// `default=[0.5, 0.5, 0.5]`）。
pub const HEX_DEFAULT_RGB: [f64; 3] = [0.5, 0.5, 0.5];

/// `UIColor.to_contrasting()` 的默认反差量（`lib/color.py:145` 的 `k=0.333`）。
pub const CONTRASTING_K: f64 = 0.333;

/// Python `int(x)`：向零截断（`to_hex_str` / `to_fill_pixel` / `UIColor.__eq__`
/// 都用它把 0..1 的通道折成 0..255）。
#[inline]
fn py_int(x: f64) -> i64 {
    x as i64
}

/// Python `round(x, 3)` 的等价物。
///
/// CPython 的 `round()` 是对**二进制真值**做十进制正确舍入（半偶），
/// 而 `(x * 1000.0).round() / 1000.0` 会先做一次二进制乘、在 2.675 这类
/// 值上给出不同结果。Rust 的 `{:.3}` 也是"对真值做十进制正确舍入、半偶"，
/// 所以借它折回 f64。
fn round3(x: f64) -> f64 {
    format!("{:.3}", x).parse().unwrap_or(x)
}

/// 三通道一起 `round3`（`__eq__` 里 `[round(c, 3) for c in t]`）。
fn round3_triple(t: [f64; 3]) -> [f64; 3] {
    [round3(t[0]), round3(t[1]), round3(t[2])]
}

/// `UIColor.__eq__`（`lib/color.py:157-172`）：两边都折成 8bpc 再比。
///
/// 源码注释说得很清楚（`:166`）："Round to 8bpc for comparison"。这是
/// **跨类**比较用的实现；同类比较由各子类自己覆写。
fn base_eq(a: &dyn UIColor, b: &dyn UIColor) -> bool {
    let q = |t: [f64; 3]| [py_int(t[0] * 255.0), py_int(t[1] * 255.0), py_int(t[2] * 255.0)];
    q(a.get_rgb()) == q(b.get_rgb())
}

/// `UIColor`：颜色对象的公共接口（`lib/color.py:36-267`）。
///
/// 默认方法就是基类的实现；四个子类按源码覆写各自该覆写的部分：
/// `get_rgb`（必须）、`get_hsv`（`HSVColor` / `HCYColor` 覆写）、
/// `get_luma`（`HCYColor` / `YCbCrColor` 覆写）、`__eq__`、`interpolate`。
pub trait UIColor {
    /// `get_rgb()`：浮点 (r, g, b)，约定各通道 0.0..=1.0。
    fn get_rgb(&self) -> [f64; 3];

    /// `get_hsv()`：浮点 (h, s, v)，`h` 是 0..1 的色相角（不是角度制）。
    ///
    /// 基类用 `colorsys.rgb_to_hsv(*self.get_rgb())`（`lib/color.py:74`）。
    fn get_hsv(&self) -> [f64; 3] {
        rgb_to_hsv(self.get_rgb())
    }

    /// `get_luma()`（`lib/color.py:114-128`）：适合绘画的感知加权亮度。
    ///
    /// 权重固定为 `0.299/0.587/0.114` —— 注意这**不是** HCY 的
    /// `0.3/0.59/0.11`（两套权重源码里就并存：前者是这里，
    /// 后者是 `_HCY_*_LUMA`，`lib/color.py:799-801`）。
    fn get_luma(&self) -> f64 {
        let [r, g, b] = self.get_rgb();
        0.299 * r + 0.587 * g + 0.114 * b
    }

    /// `to_greyscale()`（`lib/color.py:130-143`）。
    fn to_greyscale(&self) -> RgbColor {
        let luma = self.get_luma();
        RgbColor::new(luma, luma, luma)
    }

    /// `to_contrasting(k)`（`lib/color.py:145-155`）：`c = (luma + k) % 1.0`。
    fn to_contrasting(&self, k: f64) -> RgbColor {
        let luma = self.get_luma();
        let c = (luma + k).rem_euclid(1.0);
        RgbColor::new(c, c, c)
    }

    /// `to_hex_str()`（`lib/color.py:205-208`）：`#RRGGBB`。
    fn to_hex_str(&self) -> String {
        self.to_hex_str_with("#")
    }

    /// `to_hex_str(prefix=...)`：前缀可换（源码默认 `#`）。
    fn to_hex_str_with(&self, prefix: &str) -> String {
        let [r, g, b] = self.get_rgb();
        format!(
            "{}{}{}{}",
            prefix,
            hex_byte(r),
            hex_byte(g),
            hex_byte(b)
        )
    }

    /// `to_fill_pixel()`（`lib/color.py:210-220`）：GdkPixbuf 的填充像素值。
    ///
    /// 纯算术（`(r<<24) | (g<<16) | (b<<8) | 0xFF`），不含任何 GTK 调用，
    /// 所以照搬；用于把颜色喂给"整块填充"的调用方。
    fn to_fill_pixel(&self) -> u32 {
        let [r, g, b] = self.get_rgb();
        let ri = py_int(r * 255.0) as u32;
        let gi = py_int(g * 255.0) as u32;
        let bi = py_int(b * 255.0) as u32;
        (ri << 24) | (gi << 16) | (bi << 8) | 0xFF
    }

    /// Python 的 `self == other`（**带方向**，见模块文档）。
    fn eq_src(&self, other: &Color) -> bool;

    /// `interpolate(other, steps)`：生成 `steps` 个中间色的序列。
    ///
    /// 基类在源码里是 `raise NotImplementedError`（`lib/color.py:264-266`），
    /// 四个子类各有一份实现。
    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color>;

    // ---- 只读属性（`lib/color.py:81-110`）----
    /// `UIColor.r`
    fn r(&self) -> f64 {
        self.get_rgb()[0]
    }
    /// `UIColor.g`
    fn g(&self) -> f64 {
        self.get_rgb()[1]
    }
    /// `UIColor.b`
    fn b(&self) -> f64 {
        self.get_rgb()[2]
    }
    /// `UIColor.h`
    fn h(&self) -> f64 {
        self.get_hsv()[0]
    }
    /// `UIColor.s`
    fn s(&self) -> f64 {
        self.get_hsv()[1]
    }
    /// `UIColor.v`
    fn v(&self) -> f64 {
        self.get_hsv()[2]
    }
}

/// `"%02x" % int(c*0xFF)`：负值按 Python 的 `%02x` 语义输出 `-` 加绝对值。
fn hex_byte(c: f64) -> String {
    let v = py_int(c * 255.0);
    if v < 0 {
        format!("-{:x}", -v)
    } else {
        format!("{:02x}", v)
    }
}

// ---------------------------------------------------------------------------
// RGBColor
// ---------------------------------------------------------------------------

/// 加色 Red/Green/Blue 表示（`lib/color.py:269-353`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbColor {
    /// 红通道，0.0..=1.0（源码 `RGBColor.r`，可直接写）。
    pub r: f64,
    /// 绿通道，0.0..=1.0（源码 `RGBColor.g`）。
    pub g: f64,
    /// 蓝通道，0.0..=1.0（源码 `RGBColor.b`）。
    pub b: f64,
}

impl RgbColor {
    /// 由三个通道构造（`RGBColor(r=, g=, b=)`，`lib/color.py:277-297`）。
    pub fn new(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b }
    }

    /// 由三元组构造（`RGBColor(rgb=(r, g, b))`）。
    pub fn from_rgb(rgb: [f64; 3]) -> Self {
        Self::new(rgb[0], rgb[1], rgb[2])
    }

    /// `RGBColor(color=other)`：取 `other.get_rgb()`（`lib/color.py:288-289`）。
    pub fn from_color(color: &Color) -> Self {
        Self::from_rgb(color.get_rgb())
    }

    /// `UIColor.new_from_hex_str()`（`lib/color.py:193-203`）：解析 `#RRGGBB`
    /// 或 `#RGB`（也接受 `0x` 前缀），失败时**不报错**、退回
    /// [`HEX_DEFAULT_RGB`]。
    ///
    /// 逐条对应源码的两条正则（`:188-191`）：
    /// - 前缀只认 `#` 与**小写** `0x`（源码正则 `^(?:#|0x)` 不含 `0X`）；
    /// - `$` 在 Python 里也匹配末尾换行前，所以剥掉**一个**尾部 `\n`；
    /// - 6 位在前、3 位在后（`([0-9a-fA-F]{2}){3}` 再 `([0-9a-fA-F]){3}`）；
    /// - 除数是 `0xFF`（6 位）或 `0xF`（3 位）。源码用 `float.fromhex(x)`
    ///   把十六进制串读成整数（`float.fromhex("ff") == 255.0`），与这里的
    ///   `u8::from_str_radix` 等价（整数 < 2^53 在 f64 里精确）。
    pub fn new_from_hex_str(s: &str) -> Self {
        Self::new_from_hex_str_default(s, HEX_DEFAULT_RGB)
    }

    /// 同 [`Self::new_from_hex_str`]，但可指定解析失败时的默认色。
    pub fn new_from_hex_str_default(s: &str, default: [f64; 3]) -> Self {
        // Python 的 `$` 允许末尾有一个换行
        let s = s.strip_suffix('\n').unwrap_or(s);
        let body = s.strip_prefix('#').or_else(|| s.strip_prefix("0x"));
        if let Some(body) = body {
            if body.len() == 6 && body.chars().all(|c| c.is_ascii_hexdigit()) {
                let byte = |i: usize| {
                    u8::from_str_radix(&body[i..i + 2], 16).expect("已校验十六进制") as f64 / 255.0
                };
                return Self::new(byte(0), byte(2), byte(4));
            }
            if body.len() == 3 && body.chars().all(|c| c.is_ascii_hexdigit()) {
                let nib = |i: usize| {
                    u8::from_str_radix(&body[i..i + 1], 16).expect("已校验十六进制") as f64 / 15.0
                };
                return Self::new(nib(0), nib(1), nib(2));
            }
        }
        Self::from_rgb(default)
    }

    /// `RGBColor.interpolate()`（`lib/color.py:309-327`）：RGB 线性插值。
    ///
    /// 源码是生成器且 `assert steps >= 3`。
    pub fn interpolate_rgb(&self, other: &RgbColor, steps: usize) -> Vec<RgbColor> {
        assert!(steps >= 3, "interpolate() 要求 steps >= 3（lib/color.py:320）");
        let mut out = Vec::with_capacity(steps);
        for step in 0..steps {
            let p = step as f64 / (steps - 1) as f64;
            out.push(RgbColor::new(
                self.r + (other.r - self.r) * p,
                self.g + (other.g - self.g) * p,
                self.b + (other.b - self.b) * p,
            ));
        }
        out
    }

    /// 源码 `__repr__`（`lib/color.py:302-307`）。
    pub fn repr(&self) -> String {
        format!(
            "<RGBColor r={:.4}, g={:.4}, b={:.4}>",
            self.r, self.g, self.b
        )
    }
}

impl UIColor for RgbColor {
    fn get_rgb(&self) -> [f64; 3] {
        [self.r, self.g, self.b]
    }

    /// `RGBColor.__eq__`（`lib/color.py:329-353`）：两边 `get_rgb()` 后逐通道
    /// `round(3)` 比较。源码里只有 `other.get_rgb()` 抛 `AttributeError` 才退回
    /// 基类实现 —— Rust 的 [`Color`] 一定有 `get_rgb`，走不到那支。
    fn eq_src(&self, other: &Color) -> bool {
        round3_triple(self.get_rgb()) == round3_triple(other.get_rgb())
    }

    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color> {
        let other = RgbColor::from_color(other);
        self.interpolate_rgb(&other, steps)
            .into_iter()
            .map(Color::Rgb)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// HSVColor
// ---------------------------------------------------------------------------

/// 柱面 Hue/Saturation/Value 表示（`lib/color.py:356-481`）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HsvColor {
    /// 色相角，缩放到 0.0..=1.0（源码 `HSVColor.h`）。
    pub h: f64,
    /// HSV 饱和度，0.0..=1.0（源码 `HSVColor.s`）。
    pub s: f64,
    /// HSV 明度，0.0..=1.0（源码 `HSVColor.v`）。
    pub v: f64,
}

impl HsvColor {
    /// 由三个分量构造（`HSVColor(h=, s=, v=)`，`lib/color.py:373-393`）。
    pub fn new(h: f64, s: f64, v: f64) -> Self {
        Self { h, s, v }
    }

    /// 由三元组构造（`HSVColor(hsv=(h, s, v))`）。
    pub fn from_hsv(hsv: [f64; 3]) -> Self {
        Self::new(hsv[0], hsv[1], hsv[2])
    }

    /// `HSVColor(color=other)`：取 `other.get_hsv()`（`lib/color.py:384-385`），
    /// **不是**经 RGB 往返。
    pub fn from_color(color: &Color) -> Self {
        Self::from_hsv(color.get_hsv())
    }

    /// `HSVColor.interpolate()`（`lib/color.py:408-444`）：HSV 插值，色相走
    /// **最短弧**。
    ///
    /// 源码的"最短角距"是两轮比较（`:435-437`）：先取 `hb - ha`，再依次试
    /// 两个跨越 0 的候选，谁绝对值小就用谁。`h = (self.h + hdelta*p) % 1.0`
    /// 用的是**未归一化**的 `self.h`（`:441`），这里一致。
    pub fn interpolate_hsv(&self, other: &HsvColor, steps: usize) -> Vec<HsvColor> {
        assert!(steps >= 3, "interpolate() 要求 steps >= 3（lib/color.py:425）");
        let ha = self.h.rem_euclid(1.0);
        let hb = other.h.rem_euclid(1.0);
        let mut hdelta = hb - ha;
        for cand in [-(ha + 1.0 - hb), hb + 1.0 - ha] {
            if cand.abs() < hdelta.abs() {
                hdelta = cand;
            }
        }
        let mut out = Vec::with_capacity(steps);
        for step in 0..steps {
            let p = step as f64 / (steps - 1) as f64;
            let h = (self.h + hdelta * p).rem_euclid(1.0);
            out.push(HsvColor::new(
                h,
                self.s + (other.s - self.s) * p,
                self.v + (other.v - self.v) * p,
            ));
        }
        out
    }

    /// 源码 `__repr__`（`lib/color.py:401-406`）。
    pub fn repr(&self) -> String {
        format!(
            "<HSVColor h={:.4}, s={:.4}, v={:.4}>",
            self.h, self.s, self.v
        )
    }
}

impl UIColor for HsvColor {
    fn get_rgb(&self) -> [f64; 3] {
        hsv_to_rgb([self.h, self.s, self.v])
    }

    fn get_hsv(&self) -> [f64; 3] {
        [self.h, self.s, self.v]
    }

    /// `HSVColor.__eq__`（`lib/color.py:446-481`）：`get_hsv()` 后 `round(3)`
    /// 比较；**两端明度都是 0 时直接判等**（"零明度下不同色相/饱和度必须相等"，
    /// `:461-468`），但明度为 1 时没有这个豁免。
    fn eq_src(&self, other: &Color) -> bool {
        let t1 = round3_triple(self.get_hsv());
        let t2 = round3_triple(other.get_hsv());
        if t1[2] == 0.0 && t2[2] == 0.0 {
            return true;
        }
        t1 == t2
    }

    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color> {
        let other = HsvColor::from_color(other);
        self.interpolate_hsv(&other, steps)
            .into_iter()
            .map(Color::Hsv)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// HCYColor
// ---------------------------------------------------------------------------

/// 柱面 Hue/Chroma/Luma 表示（`lib/color.py:484-638`）。
///
/// 色相 `h` 与 [`HsvColor`] 同义；`c` 是"该 h/y 下允许的最大彩度的比例"
/// （所以范围是 0..=2，不是 0..=1）；`y` 是感知加权亮度（HCY 自己的权重）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HcyColor {
    /// 色相角，0.0..=1.0（源码 `HCYColor.h`）。
    pub h: f64,
    /// HCY 彩度，0.0..=2.0（源码 `HCYColor.c`）。
    pub c: f64,
    /// HCY 亮度，0.0..=1.0（源码 `HCYColor.y`）。
    pub y: f64,
}

impl HcyColor {
    /// 由三个分量构造（`HCYColor(h=, c=, y=)`，`lib/color.py:509-539`）。
    pub fn new(h: f64, c: f64, y: f64) -> Self {
        Self { h, c, y }
    }

    /// 由三元组构造（`HCYColor(hcy=(h, c, y))`）。
    pub fn from_hcy(hcy: [f64; 3]) -> Self {
        Self::new(hcy[0], hcy[1], hcy[2])
    }

    /// `HCYColor(color=other)`（`lib/color.py:521-531`）。
    ///
    /// 两条分支照抄：
    /// - 来源**已经是** `HCYColor` → 三个分量原样搬；
    /// - 否则先 `other.get_hsv()`，再 `RGB_to_HCY(colorsys.hsv_to_rgb(h, s, v))`；
    ///   若"纯黑但饱和度不为 0"，用 `v = 0.01` 重算一次 `c`，让走向黑时不丢
    ///   彩度（`:529-531`）。
    pub fn from_color(color: &Color) -> Self {
        if let Color::Hcy(hcy) = color {
            return *hcy;
        }
        let [h, s, v] = color.get_hsv();
        let hcy = rgb_to_hcy(hsv_to_rgb([h, s, v]));
        let mut c = hcy[1];
        let y = hcy[2];
        if v == 0.0 && c == 0.0 && s != 0.0 {
            c = rgb_to_hcy(hsv_to_rgb([h, s, 0.01]))[1];
        }
        HcyColor::new(h, c, y)
    }

    /// `HCYColor.interpolate()`（`lib/color.py:562-599`）：与 HSV 同样走最短弧，
    /// 但 `c`/`y` 线性 —— 同彩度两端的插值会保持彩度（源码 docstring `:572-575`）。
    pub fn interpolate_hcy(&self, other: &HcyColor, steps: usize) -> Vec<HcyColor> {
        assert!(steps >= 3, "interpolate() 要求 steps >= 3（lib/color.py:585）");
        let ha = self.h.rem_euclid(1.0);
        let hb = other.h.rem_euclid(1.0);
        let mut hdelta = hb - ha;
        for cand in [-(ha + 1.0 - hb), hb + 1.0 - ha] {
            if cand.abs() < hdelta.abs() {
                hdelta = cand;
            }
        }
        let mut out = Vec::with_capacity(steps);
        for step in 0..steps {
            let p = step as f64 / (steps - 1) as f64;
            let h = (self.h + hdelta * p).rem_euclid(1.0);
            out.push(HcyColor::new(
                h,
                self.c + (other.c - self.c) * p,
                self.y + (other.y - self.y) * p,
            ));
        }
        out
    }

    /// 源码 `__repr__`（`lib/color.py:555-560`）。
    pub fn repr(&self) -> String {
        format!(
            "<HCYColor h={:.4}, c={:.4}, y={:.4}>",
            self.h, self.c, self.y
        )
    }
}

impl UIColor for HcyColor {
    fn get_rgb(&self) -> [f64; 3] {
        hcy_to_rgb([self.h, self.c, self.y])
    }

    /// `HCYColor.get_hsv()`（`lib/color.py:541-547`）：色相直接用存下来的 `h`
    /// （不经 RGB 往返）；纯黑但彩度不为 0 时用 `y = 0.01` 补出饱和度。
    fn get_hsv(&self) -> [f64; 3] {
        let mut hsv = rgb_to_hsv(self.get_rgb());
        if hsv[1] == 0.0 && hsv[2] == 0.0 && self.c != 0.0 {
            hsv[1] = rgb_to_hsv(hcy_to_rgb([self.h, self.c, 0.01]))[1];
        }
        [self.h, hsv[1], hsv[2]]
    }

    /// `HCYColor.get_luma()`（`lib/color.py:552-553`）：就是这个 `y`。
    fn get_luma(&self) -> f64 {
        self.y
    }

    /// `HCYColor.__eq__`（`lib/color.py:601-638`）：只与 `HCYColor` 比
    /// `(h, c, y)` 的 `round(3)`；**`y == 0` 或 `y == 1` 时直接判等**
    /// （"纯黑/纯白下不同色相或饱和度必须相等"，`:616-623`）。
    /// 与别的类比时源码会因 `other.c` 抛 `AttributeError` → 退回 8bpc 比较。
    fn eq_src(&self, other: &Color) -> bool {
        let Color::Hcy(o) = other else {
            return base_eq(self, other);
        };
        let t1 = round3_triple([self.h, self.c, self.y]);
        let t2 = round3_triple([o.h, o.c, o.y]);
        if t1[2] == 0.0 && t2[2] == 0.0 {
            return true;
        }
        if t1[2] == 1.0 && t2[2] == 1.0 {
            return true;
        }
        t1 == t2
    }

    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color> {
        let other = HcyColor::from_color(other);
        self.interpolate_hcy(&other, steps)
            .into_iter()
            .map(Color::Hcy)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// YCbCrColor
// ---------------------------------------------------------------------------

/// BT601 Y'CbCr 表示（`lib/color.py:641-742`）。
///
/// `Y` 是 0..=1 的 luma，`Cb`/`Cr` 是 -0.5..=0.5 的色差分量（源码 `:644-646`）。
/// 这个空间是从 RGB 派生出来的，但立方体与它的轴不平行，所以分量一动就容易
/// 出显示色域（`:649-652`）—— [`Self::get_rgb`] **不夹取**，如实给出域外值。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YcbcrColor {
    /// BT601 luma，0.0..=1.0（源码 `YCbCrColor.Y`）。
    pub y: f64,
    /// BT601 蓝色差色度，-0.5..=0.5（源码 `YCbCrColor.Cb`）。
    pub cb: f64,
    /// BT601 红色差色度，-0.5..=0.5（源码 `YCbCrColor.Cr`）。
    pub cr: f64,
}

impl YcbcrColor {
    /// 由三个分量构造（`YCbCrColor(Y=, Cb=, Cr=)`，`lib/color.py:656-674`）。
    pub fn new(y: f64, cb: f64, cr: f64) -> Self {
        Self { y, cb, cr }
    }

    /// 由三元组构造（`YCbCrColor(YCbCr=(Y, Cb, Cr))`）。
    pub fn from_ycbcr(ycbcr: [f64; 3]) -> Self {
        Self::new(ycbcr[0], ycbcr[1], ycbcr[2])
    }

    /// `YCbCrColor(color=other)`（`lib/color.py:659-666`）：来源已是同类就原样搬，
    /// 否则 `RGB_to_YCbCr_BT601(other.get_rgb())`。
    pub fn from_color(color: &Color) -> Self {
        if let Color::Ycbcr(ycc) = color {
            return *ycc;
        }
        Self::from_ycbcr(rgb_to_ycbcr_bt601(color.get_rgb()))
    }

    /// `YCbCrColor.interpolate()`（`lib/color.py:690-716`）：三个分量都线性
    /// （此空间是 RGB 立方体的线性变换，所以插值结果与 RGB 空间几乎一致，
    /// 源码 docstring `:698-700`）。
    pub fn interpolate_ycbcr(&self, other: &YcbcrColor, steps: usize) -> Vec<YcbcrColor> {
        assert!(steps >= 3, "interpolate() 要求 steps >= 3（lib/color.py:708）");
        let mut out = Vec::with_capacity(steps);
        for step in 0..steps {
            let p = step as f64 / (steps - 1) as f64;
            out.push(YcbcrColor::new(
                self.y + (other.y - self.y) * p,
                self.cb + (other.cb - self.cb) * p,
                self.cr + (other.cr - self.cr) * p,
            ));
        }
        out
    }

    /// 源码 `__repr__`（`lib/color.py:683-688`）。
    pub fn repr(&self) -> String {
        format!(
            "<YCbCrColor Y={:.4}, Cb={:.4}, Cr={:.4}>",
            self.y, self.cb, self.cr
        )
    }
}

impl UIColor for YcbcrColor {
    /// `YCbCrColor.get_rgb()`（`lib/color.py:679-681`）："possibly out of gamut"。
    fn get_rgb(&self) -> [f64; 3] {
        ycbcr_to_rgb_bt601([self.y, self.cb, self.cr])
    }

    /// `YCbCrColor.get_luma()`（`lib/color.py:676-677`）：就是这个 `Y`。
    fn get_luma(&self) -> f64 {
        self.y
    }

    /// `YCbCrColor.__eq__`（`lib/color.py:718-742`）：只与 `YCbCrColor` 比
    /// `(Y, Cb, Cr)` 的 `round(3)`；与别的类比时因 `other.Y` 抛
    /// `AttributeError` → 退回 8bpc 比较。
    fn eq_src(&self, other: &Color) -> bool {
        let Color::Ycbcr(o) = other else {
            return base_eq(self, other);
        };
        round3_triple([self.y, self.cb, self.cr]) == round3_triple([o.y, o.cb, o.cr])
    }

    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color> {
        let other = YcbcrColor::from_color(other);
        self.interpolate_ycbcr(&other, steps)
            .into_iter()
            .map(Color::Ycbcr)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Color（带类型标签的 UIColor 持有者）
// ---------------------------------------------------------------------------

/// 任意一种 `UIColor` —— 源码里"中央共享颜色"的持有类型。
///
/// 类型标签必须留着：`__eq__` 按左操作数的类分派，`get_hsv()` 在
/// `HCYColor` 上有纯黑特例（`lib/color.py:541-547`），`interpolate()` 也在
/// 各自的色彩空间里做。把颜色折成 RGB 元组存会丢这些语义。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Color {
    /// `RGBColor`
    Rgb(RgbColor),
    /// `HSVColor`
    Hsv(HsvColor),
    /// `HCYColor`
    Hcy(HcyColor),
    /// `YCbCrColor`
    Ycbcr(YcbcrColor),
}

impl Color {
    /// `copy.copy(color)`：源码 `get_color()` 返回的是一份拷贝
    /// （`gui/colors/adjbases.py:222-224`）。[`Color`] 是 `Copy`，所以这是恒等操作。
    pub fn copied(self) -> Color {
        self
    }

    /// 源码 `__repr__`（四个子类各自的实现）。
    pub fn repr(&self) -> String {
        match self {
            Color::Rgb(c) => c.repr(),
            Color::Hsv(c) => c.repr(),
            Color::Hcy(c) => c.repr(),
            Color::Ycbcr(c) => c.repr(),
        }
    }

    /// 类名（源码 `type(color).__name__`），用于日志与测试报错。
    pub fn class_name(&self) -> &'static str {
        match self {
            Color::Rgb(_) => "RGBColor",
            Color::Hsv(_) => "HSVColor",
            Color::Hcy(_) => "HCYColor",
            Color::Ycbcr(_) => "YCbCrColor",
        }
    }

    /// `HCYColor(color=...)` 的等价值（源码没有 `get_hcy()` 方法，只有这个
    /// 构造路径 —— 见 `lib/color.py:521-531`）。
    pub fn get_hcy(&self) -> [f64; 3] {
        let c = HcyColor::from_color(self);
        [c.h, c.c, c.y]
    }

    /// `YCbCrColor(color=...)` 的等价值（`lib/color.py:659-666`）。
    pub fn get_ycbcr(&self) -> [f64; 3] {
        let c = YcbcrColor::from_color(self);
        [c.y, c.cb, c.cr]
    }
}

impl UIColor for Color {
    fn get_rgb(&self) -> [f64; 3] {
        match self {
            Color::Rgb(c) => c.get_rgb(),
            Color::Hsv(c) => c.get_rgb(),
            Color::Hcy(c) => c.get_rgb(),
            Color::Ycbcr(c) => c.get_rgb(),
        }
    }

    fn get_hsv(&self) -> [f64; 3] {
        match self {
            Color::Rgb(c) => c.get_hsv(),
            Color::Hsv(c) => c.get_hsv(),
            Color::Hcy(c) => c.get_hsv(),
            Color::Ycbcr(c) => c.get_hsv(),
        }
    }

    fn get_luma(&self) -> f64 {
        match self {
            Color::Rgb(c) => c.get_luma(),
            Color::Hsv(c) => c.get_luma(),
            Color::Hcy(c) => c.get_luma(),
            Color::Ycbcr(c) => c.get_luma(),
        }
    }

    fn eq_src(&self, other: &Color) -> bool {
        match self {
            Color::Rgb(c) => c.eq_src(other),
            Color::Hsv(c) => c.eq_src(other),
            Color::Hcy(c) => c.eq_src(other),
            Color::Ycbcr(c) => c.eq_src(other),
        }
    }

    fn interpolate(&self, other: &Color, steps: usize) -> Vec<Color> {
        match self {
            Color::Rgb(c) => c.interpolate(other, steps),
            Color::Hsv(c) => c.interpolate(other, steps),
            Color::Hcy(c) => c.interpolate(other, steps),
            Color::Ycbcr(c) => c.interpolate(other, steps),
        }
    }
}

/// `c == color or color == c` —— `push_history()` 的去重判据
/// （`gui/colors/adjbases.py:249-255`）。
///
/// 源码注释专门解释了为什么要**两个方向都试**：
/// "Direction of the comparison can matter now that color classes have
/// overridden `__eq__` methods."
pub fn colors_match(a: &Color, b: &Color) -> bool {
    a.eq_src(b) || b.eq_src(a)
}
