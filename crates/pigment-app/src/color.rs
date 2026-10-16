//! 颜色管理 —— 照搬 MyPaint `gui/colors/adjbases.py` 的 `ColorManager` 核心。
//!
//! MyPaint 的颜色由**应用**持有，再推给笔刷的 `color_h/s/v`
//! （`gui/brushcolor.py:44` `BrushColorManager.set_color` → `brush.set_color_hsv`），
//! 并在**每一笔结束后**把当前色压入颜色历史
//! （`gui/brushcolor.py:54` `__input_stroke_ended_cb`，橡皮不记录）。
//!
//! 本模块只搬「颜色历史」这一块 —— 长度 5、重复色去重后移到末尾，
//! 与 `push_history()`（`adjbases.py:240`）逐条对应。
//!
//! 与 Python 的唯一差异：浮点相等改成带容差比较（MyPaint 比的是
//! `RGBColor` namedtuple 的精确相等），否则滑块产生的微小差异会让去重失效。

/// 颜色历史长度（`adjbases.py:79` `_HIST_LEN = 5`）。
pub const HIST_LEN: usize = 5;

/// 默认颜色历史（`adjbases.py:78` `_DEFAULT_HIST`），顺序 = 最旧 → 最新。
///
/// 没有持久化偏好时，启动的当前颜色就是**末项** `#003153`
/// （`adjbases.py:129-131`：`col_hex = hist_hex[-1]`）。
pub const DEFAULT_HIST_HEX: [&str; HIST_LEN] =
    ["#ee3333", "#336699", "#44aa66", "#aa6633", "#003153"];

/// 去重容差（HSV 各分量）。
const EPS: f32 = 1e-4;

/// RGB（各分量 0..1）→ HSV。对齐 `lib/helpers.py:rgb_to_hsv`（即 `colorsys.rgb_to_hsv`）。
pub fn rgb_to_hsv(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = [
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
    ];
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
    let h = if r == maxc {
        bc - gc
    } else if g == maxc {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    [(h / 6.0).rem_euclid(1.0), s, v]
}

/// HSV → RGB。对齐 `lib/helpers.py:hsv_to_rgb`（即 `colorsys.hsv_to_rgb`）。
pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [h, s, v] = [
        hsv[0].clamp(0.0, 1.0),
        hsv[1].clamp(0.0, 1.0),
        hsv[2].clamp(0.0, 1.0),
    ];
    if s == 0.0 {
        return [v, v, v];
    }
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match (i as i32).rem_euclid(6) {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// `#rrggbb` → HSV。对齐 `RGBColor.new_from_hex_str()` + `get_hsv()`。
pub fn hsv_from_hex(s: &str) -> Option<[f32; 3]> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok().map(|b| b as f32 / 255.0);
    Some(rgb_to_hsv([byte(0)?, byte(2)?, byte(4)?]))
}

/// HSV → `#rrggbb`（MyPaint 的颜色详情面板用同一格式显示）。
pub fn hex_from_hsv(hsv: [f32; 3]) -> String {
    let [r, g, b] = hsv_to_rgb(hsv);
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", q(r), q(g), q(b))
}

/// 颜色历史（`ColorManager._hist`）。
#[derive(Debug, Clone)]
pub struct ColorHistory {
    colors: Vec<[f32; 3]>,
}

impl Default for ColorHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorHistory {
    /// 启动时的历史 = `_DEFAULT_HIST`（我们还没做偏好持久化，
    /// MyPaint 会在此之上追加 `colors.history` 里的用户历史）。
    pub fn new() -> Self {
        Self {
            colors: DEFAULT_HIST_HEX
                .iter()
                .filter_map(|s| hsv_from_hex(s))
                .collect(),
        }
    }

    /// 当前颜色 = 历史末项（`adjbases.py:131` `col_hex = hist_hex[-1]`）。
    pub fn current(&self) -> [f32; 3] {
        *self.colors.last().expect("颜色历史非空")
    }

    /// 对齐 `push_history()`：重复色先删掉，再追加到末尾，最后裁到 `_HIST_LEN`。
    pub fn push(&mut self, color: [f32; 3]) {
        self.colors.retain(|c| !near(*c, color));
        self.colors.push(color);
        let n = self.colors.len();
        if n > HIST_LEN {
            self.colors.drain(0..n - HIST_LEN);
        }
    }

    /// 历史快照（最旧 → 最新）。
    pub fn colors(&self) -> &[[f32; 3]] {
        &self.colors
    }
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < EPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_defaults_match_mypaint() {
        // 逐个人工核对 colorsys 的输出
        let red = hsv_from_hex("#ee3333").unwrap();
        assert!((red[0] - 0.0).abs() < 1e-6, "色相 0");
        assert!((red[1] - 0.7857).abs() < 1e-3, "饱和 0.7857");
        assert!((red[2] - 0.9333).abs() < 1e-3, "明度 0.9333");

        let navy = hsv_from_hex("#003153").unwrap();
        assert!((navy[0] - 0.5684).abs() < 1e-3, "色相 ≈0.5684");
        assert!((navy[1] - 1.0).abs() < 1e-6, "饱和 1.0");
        assert!((navy[2] - 0.3255).abs() < 1e-3, "明度 0.3255");
    }

    #[test]
    fn hsv_and_rgb_round_trip() {
        for hex in DEFAULT_HIST_HEX {
            let hsv = hsv_from_hex(hex).unwrap();
            let rgb = hsv_to_rgb(hsv);
            let back = rgb_to_hsv(rgb);
            for i in 0..3 {
                assert!((hsv[i] - back[i]).abs() < 1e-4, "{hex} 分量 {i} 往返失败");
            }
            // HSV 往返后仍能还原出原十六进制串
            assert_eq!(hex_from_hsv(hsv), hex, "{hex} 十六进制往返失败");
        }
    }

    #[test]
    fn startup_color_is_last_default() {
        // MyPaint 启动色 = _DEFAULT_HIST 末项 #003153
        let h = ColorHistory::new();
        assert_eq!(h.colors().len(), HIST_LEN);
        assert!(near(h.current(), hsv_from_hex("#003153").unwrap()));
    }

    #[test]
    fn push_history_dedupes_and_keeps_mru_last() {
        let mut h = ColorHistory::new();
        let first = h.colors()[0];
        // 压入一个已经在历史里的颜色 → 它应被移到末尾，长度不变
        h.push(first);
        assert_eq!(h.colors().len(), HIST_LEN);
        assert!(near(h.current(), first));
        assert_eq!(h.colors().iter().filter(|c| near(**c, first)).count(), 1);
        // 压入新颜色 → 末尾为它，最旧的被挤掉
        let newest = [0.5, 0.5, 0.5];
        let oldest_before = h.colors()[0];
        h.push(newest);
        assert_eq!(h.colors().len(), HIST_LEN);
        assert!(near(h.current(), newest));
        assert!(!h.colors().iter().any(|c| near(*c, oldest_before)));
    }
}
