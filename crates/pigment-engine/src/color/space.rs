//! 色彩空间换算 —— `lib/color.py` 与标准库 `colorsys` 的逐行移植。
//!
//! 全部用 `f64`：Python 的 `float` 就是 IEEE-754 双精度，所以这里的每一步都能
//! 与源码**逐位**对照 —— `crates/pigment-engine/tests/color_vectors.rs` 用真
//! `lib/color.py` 算出的向量断言 `==`（fixture 由
//! `.workbuddy/tools/gen-color-vectors.py` 生成）。
//!
//! 与源码一致，**任何一处都不做夹取**：`colorsys` 不夹取，`lib/color.py` 也不
//! 夹取（`YCbCrColor.get_rgb()` 的文档明说"possibly out of gamut"，`lib/color.py:680`）。

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// HCY 的红色 luma 权重 —— `lib/color.py:799` 的 `_HCY_RED_LUMA`。
///
/// 源码注释（`:794-797`）说明这三者刻意与图层混合模式 Color/Luminosity 和
/// brushlib 的 Colorize 一致，遵循 <http://www.w3.org/TR/compositing/>。
pub const HCY_RED_LUMA: f64 = 0.3;
/// HCY 的绿色 luma 权重 —— `lib/color.py:800` 的 `_HCY_GREEN_LUMA`。
pub const HCY_GREEN_LUMA: f64 = 0.59;
/// HCY 的蓝色 luma 权重 —— `lib/color.py:801` 的 `_HCY_BLUE_LUMA`。
pub const HCY_BLUE_LUMA: f64 = 0.11;

// ---------------------------------------------------------------------------
// RGB ↔ HSV（`colorsys`）
// ---------------------------------------------------------------------------

/// `colorsys.rgb_to_hsv` —— `UIColor.get_hsv()`（`lib/color.py:65-74`）就是直接
/// 调它，`HSVColor` 之外的三个子类都继承这一实现。
///
/// 逐行照搬 CPython `Lib/colorsys.py`：无彩色时 `h = 0`；`h` 最后做
/// `(h/6.0) % 1.0`（Python 的 `%` 是向下取整语义，故用 [`f64::rem_euclid`]）。
pub fn rgb_to_hsv(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let maxc = r.max(g).max(b);
    let minc = r.min(g).min(b);
    let rangec = maxc - minc;
    let v = maxc;
    if minc == maxc {
        return [0.0, 0.0, v];
    }
    let s = rangec / maxc;
    let rc = (maxc - r) / rangec;
    let gc = (maxc - g) / rangec;
    let bc = (maxc - b) / rangec;
    let h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    [(h / 6.0).rem_euclid(1.0), s, v]
}

/// `colorsys.hsv_to_rgb` —— `HSVColor.get_rgb()`（`lib/color.py:398-399`）。
///
/// 注意源码用的是 `i = int(h * 6.0)`（**向零截断**）再 `i = i % 6`
/// （Python 的 `%`，结果非负），这里逐条对应：先 `trunc()`，再
/// `rem_euclid(6.0)`，`f` 用未取模的 `h*6.0 - i`。
pub fn hsv_to_rgb(hsv: [f64; 3]) -> [f64; 3] {
    let [h, s, v] = hsv;
    if s == 0.0 {
        return [v, v, v];
    }
    let h6 = h * 6.0;
    let i = h6.trunc(); // int(h*6.0)
    let f = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    // i % 6（Python 语义），取值必在 0..=5
    match i.rem_euclid(6.0) as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

// ---------------------------------------------------------------------------
// RGB ↔ BT601 Y'CbCr
// ---------------------------------------------------------------------------

/// `RGB_to_YCbCr_BT601`（`lib/color.py:759-765`）：R,G,B,Y ∈ [0,1]，
/// Cb,Cr ∈ [-0.5, 0.5]。
pub fn rgb_to_ycbcr_bt601(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = -0.169 * r - 0.331 * g + 0.500 * b;
    let cr = 0.500 * r - 0.419 * g - 0.081 * b;
    [y, cb, cr]
}

/// `YCbCr_to_RGB_BT601`（`lib/color.py:768-774`）。
///
/// 源码把 Cb/Cr 叫 `U`/`V`（`:770`），结果**可能超出色域**（`YCbCrColor.get_rgb`
/// 的文档就是这么说的，`lib/color.py:680`），这里如实照抄、不夹取。
pub fn ycbcr_to_rgb_bt601(ycbcr: [f64; 3]) -> [f64; 3] {
    let [y, u, v] = ycbcr;
    let r = y + 1.403 * v;
    let g = y - 0.344 * u - 0.714 * v;
    let b = y + 1.773 * u;
    [r, g, b]
}

// ---------------------------------------------------------------------------
// RGB ↔ HCY
// ---------------------------------------------------------------------------

/// `RGB_to_HCY`（`lib/color.py:804-841`）：返回 `[h, c, y]`，其中 `h ∈ [0,1)`，
/// 但 **`c ∈ [0,2]`**、`y ∈ [0,1]`（源码 docstring `:809` 明说）。
///
/// 逐行照搬，包括三处容易写错的细节：
/// - 色相按"最大通道"分扇区，`p == r` 且 `g < b` 时 `h += 6.0` 绕回（`:825-827`）；
/// - 无彩色（`r == g == b`）时 `c = 0`，避免 `(y-n)/y` 除零（`:835-837`）；
/// - `c` 取 `max((y-n)/y, (p-y)/(1-y))`（GLHS 论文的推导，`:839-840`）。
pub fn rgb_to_hcy(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    // luma：三通道加权和
    let y = HCY_RED_LUMA * r + HCY_GREEN_LUMA * g + HCY_BLUE_LUMA * b;
    // 色相：先按最大通道分扇区，再加另两个通道的差
    let p = r.max(g).max(b);
    let n = r.min(g).min(b);
    let d = p - n; // 绝对彩度，只用于归一化
    let mut h = if n == p {
        0.0
    } else if p == r {
        let h = (g - b) / d;
        if h < 0.0 {
            h + 6.0
        } else {
            h
        }
    } else if p == g {
        ((b - r) / d) + 2.0
    } else {
        // p == b
        ((r - g) / d) + 4.0
    };
    h /= 6.0;
    // 彩度：相对 RGB 色域包络的比例
    let c = if r == g && g == b {
        0.0
    } else {
        ((y - n) / y).max((p - y) / (1.0 - y))
    };
    [h, c, y]
}

/// `HCY_to_RGB`（`lib/color.py:844-916`）：`[h, c, y] → [r, g, b]`，恒在 [0,1] 内。
///
/// 逐行照搬六个扇区的排序技巧：先按扇区算出 `(p, o, n)` 三个"排序后"的分量，
/// 再按扇区还原成 RGB 顺序。源码的 `h %= 1.0` 是 Python 语义，用
/// [`f64::rem_euclid`]。
pub fn hcy_to_rgb(hcy: [f64; 3]) -> [f64; 3] {
    let [h0, c, y] = hcy;
    // `c == 0` 是无彩色，直接返回灰（`:864-865`）
    if c == 0.0 {
        return [y, y, y];
    }
    let h = h0.rem_euclid(1.0) * 6.0; // 源码 `h %= 1.0; h *= 6.0`
                                      // 每个扇区给出 th（第三个分量的位置）与 tm（中间分量的位置）
    let (sector, th, tm) = if h < 1.0 {
        (0usize, h, HCY_RED_LUMA + HCY_GREEN_LUMA * h)
    } else if h < 2.0 {
        let th = 2.0 - h;
        (1, th, HCY_GREEN_LUMA + HCY_RED_LUMA * th)
    } else if h < 3.0 {
        let th = h - 2.0;
        (2, th, HCY_GREEN_LUMA + HCY_BLUE_LUMA * th)
    } else if h < 4.0 {
        let th = 4.0 - h;
        (3, th, HCY_BLUE_LUMA + HCY_GREEN_LUMA * th)
    } else if h < 5.0 {
        let th = h - 4.0;
        (4, th, HCY_BLUE_LUMA + HCY_RED_LUMA * th)
    } else {
        let th = 6.0 - h;
        (5, th, HCY_RED_LUMA + HCY_BLUE_LUMA * th)
    };
    // 先算出排序后的 (p, o, n)（`:894-902`）
    let (p, o, n) = if tm >= y {
        (
            y + y * c * (1.0 - tm) / tm,
            y + y * c * (th - tm) / tm,
            y - (y * c),
        )
    } else {
        (
            y + (1.0 - y) * c,
            y + (1.0 - y) * c * (th - tm) / (1.0 - tm),
            y - (1.0 - y) * c * tm / (1.0 - tm),
        )
    };
    // 再按扇区还原成 RGB 顺序（`:904-916`）
    match sector {
        0 => [p, o, n],
        1 => [o, p, n],
        2 => [n, p, o],
        3 => [n, o, p],
        4 => [o, n, p],
        _ => [p, n, o],
    }
}
