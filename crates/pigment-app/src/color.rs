//! 应用层的颜色适配 —— 引擎 `pigment_engine::color` 的 `[f32; 3]` HSV 外观。
//!
//! **B4-1 起，颜色的唯一事实来源是引擎**：
//!
//! - `pigment_engine::color` 里是 `lib/color.py` 的 `UIColor` 类层次与全部换算
//!   （`RgbColor`/`HsvColor`/`HcyColor`/`YcbcrColor`、BT601/HCY 换算、
//!   `to_hex_str`、`new_from_hex_str`、`to_fill_pixel`、`interpolate`、`__eq__`）；
//!   `ColorManager` 是 `gui/colors/adjbases.py:ColorManager` 的数据侧 +
//!   `gui/brushcolor.py:BrushColorManager` 的语义（当前色 / 历史 / 调整动作 /
//!   `misc.context_restores_color`）。
//! - **本文件不再持有任何颜色状态，也不再实现换算** —— 之前那份 f32 的
//!   `rgb_to_hsv`/`hsv_to_rgb`/`ColorHistory` 已经删掉，避免出现第二套语义。
//!
//! 这里只剩两件工程上的事：
//!
//! 1. `[f32; 3]` HSV ↔ 引擎 f64 类型的**边界转换**。用 f32 是因为笔刷通道
//!    本身就是 C `float`（libmypaint 的 brush setting 是 float），egui 的
//!    `Slider`/`Color32` 也是 f32；颜色的**语义**（比较、夹取、换算）全在 f64 的
//!    引擎侧做，与 Python 的 double 逐位对齐。
//! 2. `#rrggbb` 的显示小工具（MyPaint 的 GUI 其实不显示十六进制，
//!    这是本移植加在悬停提示里的信息），实现转发到引擎的
//!    [`pigment_engine::color::UIColor::to_hex_str`]。

use pigment_engine::color::{Color, HsvColor, UIColor};

/// `[f32; 3]` → `[f64; 3]`（笔刷通道 → 颜色模型）。
pub fn to_f64_3(v: [f32; 3]) -> [f64; 3] {
    [v[0] as f64, v[1] as f64, v[2] as f64]
}

/// `[f64; 3]` → `[f32; 3]`（颜色模型 → 笔刷通道 / egui 控件）。
pub fn to_f32_3(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

/// 颜色的 HSV 三元组（`get_hsv()`），转成笔刷通道用的 `[f32; 3]`。
pub fn hsv_of(color: &Color) -> [f32; 3] {
    to_f32_3(color.get_hsv())
}

/// `[f32; 3]` HSV → 引擎的 `HSVColor`。
///
/// 笔刷的 `color_h/s/v`、取色器的出口、滑块的值都是 HSV，所以从这些地方进
/// 管理器时类型就是 `HSVColor` —— 与 `BrushColorManager.__settings_changed_cb`
/// 的 `lib.color.HSVColor(*self.__brush.get_color_hsv())`（`gui/brushcolor.py:43`）
/// 一致。
pub fn hsv_color(hsv: [f32; 3]) -> Color {
    Color::Hsv(HsvColor::from_hsv(to_f64_3(hsv)))
}

/// HSV → RGB（`colorsys.hsv_to_rgb` 的移植，在引擎里）。显示用。
pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    to_f32_3(pigment_engine::color::hsv_to_rgb(to_f64_3(hsv)))
}

/// `#rrggbb` 显示（`UIColor.to_hex_str()`，`lib/color.py:205-208`）。
///
/// 注意源码用的是 `int(c * 0xFF)`（**截断**），不是四舍五入 —— 所以一个从
/// `#ee3333` 解析出来、又在 HSV 里存了一道的颜色，写回来是 `#ee3232`
/// （HSV 往返丢了 1/255）。这是源码行为，不是本移植的取舍。
pub fn hex_from_hsv(hsv: [f32; 3]) -> String {
    // 刻意**不**经过 f32 的 RGB：源码是在 f64 里算完再截断的
    // （`HSVColor.get_rgb()` → `int(c*0xFF)`），中间夹一次 f32 会把
    // 0.19999999999999998 抬成 0.2，让本该是 `#ee3232` 的结果变成 `#ee3333`。
    let color = Color::Hsv(HsvColor::from_hsv(to_f64_3(hsv)));
    color.to_hex_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::color::RgbColor;

    /// 引擎侧的 hex 写回是"截断"，与源码一致。
    ///
    /// `to_hex_str()` 用的是 `int(c * 0xFF)`（`lib/color.py:205-208`），
    /// 不是四舍五入 —— v=0.9999 的灰落在 254.97/255，写出来是 `#fefefe`
    /// 而不是 `#ffffff`。四舍五入的实现会在这里挂掉。
    #[test]
    fn hex_from_hsv_truncates_like_source() {
        assert_eq!(hex_from_hsv([0.0, 0.0, 0.9999]), "#fefefe");
    }

    /// 笔刷通道是 C `float`，所以"hex → HSV(f32) → RGB → hex"可能丢 1/255。
    ///
    /// 这与源码同构：`color_h/s/v` 在 libmypaint 里就是 `float`
    /// （`mypaint-brush.c` 的 `base_values`），MyPaint 的 `to_hex_str()`
    /// 也是在这个精度上做 `int(c*0xFF)`。`#336699` 的红通道落到 50.99999，
    /// 截断成 50（`0x32`）—— 换成四舍五入会得到 51，那才是不符合源码的。
    #[test]
    fn f32_channel_may_lose_one_step_of_255() {
        assert_eq!(hex_from_hsv(hsv_color_of_hex("#336699")), "#326699");
        // 精度足够的颜色不受影响
        assert_eq!(hex_from_hsv(hsv_color_of_hex("#003153")), "#003153");
        assert_eq!(hex_from_hsv(hsv_color_of_hex("#ffffff")), "#ffffff");
    }

    /// 测试助手：`#rrggbb` → HSV `[f32; 3]`（走引擎的 hex 解析 + `get_hsv()`）。
    fn hsv_color_of_hex(s: &str) -> [f32; 3] {
        let rgb = RgbColor::new_from_hex_str(s);
        hsv_of(&Color::Rgb(rgb))
    }

    /// `[f32; 3]` ↔ f64 的边界转换是无损的（f32 值在 f64 里精确）。
    #[test]
    fn f32_f64_boundary_is_lossless() {
        let v = [0.1f32, 0.5, 0.9999];
        assert_eq!(to_f32_3(to_f64_3(v)), v);
    }

    /// 从笔刷通道进来的颜色是 `HSVColor`，`get_hsv()` 不做任何换算。
    #[test]
    fn hsv_color_round_trips_through_manager_type() {
        let hsv = [0.25f32, 0.5, 0.75];
        let c = hsv_color(hsv);
        assert_eq!(c.class_name(), "HSVColor");
        assert_eq!(hsv_of(&c), hsv);
    }
}
