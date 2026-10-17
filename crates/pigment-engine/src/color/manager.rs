//! 颜色管理器 —— `gui/colors/adjbases.py` 的 `ColorManager` 数据部分
//! + `gui/brushcolor.py` 的 `BrushColorManager` 语义。
//!
//! ## 源码里这块是怎么分工的
//!
//! - `ColorManager`（`adjbases.py:62-359`）持有三样东西：当前色 `_color`
//!   （一个 `UIColor`）、颜色历史 `_hist`（长度 5，最近用的在最后）、
//!   当前调色板 `_palette`；并负责把颜色变化通知已注册的 adjuster。
//! - `BrushColorManager`（`brushcolor.py:16-71`）在 `ColorManager` 之上加三件事：
//!   1. `set_color()` 之后把颜色推给笔刷的 `color_h/s/v`（`:33-37`）；
//!   2. 笔刷设置（含外部来源）变了就回填管理器（`:39-48`）；
//!   3. 每一笔结束、以及每次 flushing sync 时把当前色压入历史
//!      （`:50-71`，橡皮不记录）。
//!
//! 本模块移植的是 **1 与 2 的数据侧 + ColorManager 的当前色/历史**；
//! 第 3 条的触发时机留在应用层（`pigment-app` 的笔画结束钩子），
//! 因为它依赖 GUI 事件流而不是颜色模型。
//!
//! ## 没有移植的部分（记账）
//!
//! - **偏好持久化**：源码从 `prefs["colors.current"]` / `prefs["colors.history"]`
//!   读、写回（`adjbases.py:121-131`、`:258-263`）。本移植还没有偏好系统
//!   （B6），所以 [`ColorManager::new`] 只按 `_DEFAULT_HIST` 播种，当前色取历史
//!   末项（照 `adjbases.py:129-131` 的 `col_hex = hist_hex[-1]` 那支）。
//!   键名常量照抄下来，等偏好系统到位直接接。
//! - **调色板**：`_palette` / `Palette` 属 B4-4，`set_color()` 里的
//!   `self._palette.match_color(color)`（`:219`）与 `_palette_changed_cb` 没有对应物。
//! - **adjuster 通知**：`color_updated` / `color_history_updated`
//!   （`lib/observable.py` 的 `@event`）是给 GUI 控件用的；egui 是立即模式，
//!   控件每帧读管理器即可（`pigment-app` 的 `shell/` 就是这么做的）。
//! - **拾色器光标** `_picker_cursor`（`adjbases.py:114-116`）、**色轮色相
//!   畸变表** `_hue_distorts` / `distort_hue()`（`:80-95`、`:289-345`）：
//!   前者是 Gdk 对象，后者服务色轮控件 —— 都属 B4-3（`gui/colors/` 控件）。

use super::adjust::{ColorAction, CONTEXT_RESTORES_COLOR_DEFAULT};
use super::model::{colors_match, Color, HsvColor, RgbColor, UIColor};

/// 颜色历史长度 —— `adjbases.py:79` 的 `_HIST_LEN = 5`。
pub const HIST_LEN: usize = 5;

/// 默认颜色历史 —— `adjbases.py:78` 的 `_DEFAULT_HIST`，顺序 = 最旧 → 最新。
///
/// 没有任何偏好时，启动的当前色就是**末项** `#003153`
/// （`adjbases.py:129-131`）。
pub const DEFAULT_HIST_HEX: [&str; HIST_LEN] =
    ["#ee3333", "#336699", "#44aa66", "#aa6633", "#003153"];

/// 偏好键：当前色（`adjbases.py:45` `PREFS_KEY_CURRENT_COLOR`）。
pub const PREFS_KEY_CURRENT_COLOR: &str = "colors.current";
/// 偏好键：颜色历史（`adjbases.py:46` `PREFS_KEY_COLOR_HISTORY`）。
pub const PREFS_KEY_COLOR_HISTORY: &str = "colors.history";
/// 偏好键：恢复笔刷槽位时是否连颜色一起还原
/// （`gui/userconfig.py:93` `misc.context_restores_color`）。
pub const PREFS_KEY_CONTEXT_RESTORES_COLOR: &str = "misc.context_restores_color";

/// 由 hex 串建颜色历史（`adjbases.py:122-125`：`_DEFAULT_HIST + 用户历史`
/// 之后逐个 `RGBColor.new_from_hex_str()`）。
fn hist_from_hex(hex: &[&str]) -> Vec<Color> {
    hex.iter()
        .map(|s| Color::Rgb(RgbColor::new_from_hex_str(s)))
        .collect()
}

/// 中央共享颜色的持有者 —— `ColorManager`（`adjbases.py:62-359`）的数据侧。
///
/// 用法与源码一致：`set_color()` 换当前色、`push_history()` 记历史、
/// 颜色调整动作走 [`ColorManager::adjust`]。
#[derive(Clone, Debug)]
pub struct ColorManager {
    /// 当前被编辑的颜色（源码 `_color`，一个 `UIColor`）。
    color: Color,
    /// 以前用过、可用于绘画的颜色，最近用的在**最后**（源码 `_hist`）。
    hist: Vec<Color>,
    /// 恢复笔刷槽位时是否连颜色一起还原（源码偏好
    /// `misc.context_restores_color`，`gui/userconfig.py:93` 默认 True）。
    context_restores_color: bool,
}

impl Default for ColorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorManager {
    /// 按源码的启动路径初始化：历史 = `_DEFAULT_HIST`，当前色 = 历史末项。
    ///
    /// 对应 `adjbases.py:121-131`（只是没有偏好字典可读，见模块文档）。
    pub fn new() -> Self {
        let hist = hist_from_hex(&DEFAULT_HIST_HEX);
        // hist_hex[-1]
        let color = *hist.last().expect("_DEFAULT_HIST 非空");
        Self {
            color,
            hist,
            context_restores_color: CONTEXT_RESTORES_COLOR_DEFAULT,
        }
    }

    // ---- 当前色（`adjbases.py:202-226`）----

    /// `set_color()`（`adjbases.py:204-220`）：颜色**没变就不动**（`:213-214`
    /// 的 `if color == self._color: return`），否则存一份拷贝。
    ///
    /// 返回是否真的变了 —— 应用层要据此决定要不要重推笔刷
    /// （`BrushColorManager.set_color` 的语义，`brushcolor.py:33-37`）。
    pub fn set_color(&mut self, color: Color) -> bool {
        // 注意方向：源码是 `color == self._color`，即**新颜色**在左。
        // 四个子类的 `__eq__` 语义不同，方向会影响结果（见 model 模块文档）。
        if color.eq_src(&self.color) {
            return false;
        }
        self.color = color.copied();
        true
    }

    /// `get_color()`（`adjbases.py:222-224`）：取当前色的一份拷贝。
    pub fn get_color(&self) -> Color {
        self.color.copied()
    }

    /// 当前色的 `get_hsv()` —— 应用层推给笔刷 `color_h/s/v` 用的那一路
    /// （`brushcolor.py:37` 的 `brush.set_color_hsv(color.get_hsv())`）。
    pub fn get_hsv(&self) -> [f64; 3] {
        self.color.get_hsv()
    }

    // ---- 颜色历史（`adjbases.py:235-281`）----

    /// `_trim_hist()`（`adjbases.py:237-238`）：只留最后 `_HIST_LEN` 个。
    fn trim_hist(&mut self) {
        let n = self.hist.len();
        if n > HIST_LEN {
            self.hist.drain(0..n - HIST_LEN);
        }
    }

    /// `push_history()`（`adjbases.py:240-266`）。
    ///
    /// 三步照抄：
    /// 1. 把"与 `color` 相等"的旧记录全删掉 —— 判据是
    ///    `not (c == color or color == c)`，**两个方向都试**（`:249-255`）；
    /// 2. 追加到末尾（最近用的在最后）；
    /// 3. 裁到 `_HIST_LEN`。
    ///
    /// 历史里**允许** `HCYColor`/`HSVColor` 等任意子类（源码也是这么存的）。
    pub fn push_history(&mut self, color: Color) {
        self.hist.retain(|c| !colors_match(c, &color));
        self.hist.push(color);
        self.trim_hist();
    }

    /// 把**当前色**压入历史（`brushcolor.py:57`/`:71` 那两处做的就是这件事）。
    pub fn push_current_history(&mut self) {
        let color = self.color.copied();
        self.push_history(color);
    }

    /// `get_history()`（`adjbases.py:275-277`）：历史的快照，最旧 → 最新。
    pub fn get_history(&self) -> &[Color] {
        &self.hist
    }

    /// 历史的 hex 形式 —— 对应 `push_history()` 写回偏好的那个列表
    /// （`adjbases.py:258-263`）。
    pub fn history_hex(&self) -> Vec<String> {
        self.hist.iter().map(|c| c.to_hex_str()).collect()
    }

    /// `get_previous_color()`（`adjbases.py:279-281`）：历史里最近用过的颜色。
    ///
    /// 底栏颜色指示器的右半就是它（`gui/colors/adjbases.py` 的
    /// `PreviousCurrentColorAdjuster.render_background_cb`）。
    /// 历史由 [`Self::new`] 播种、裁切后长度恒在 `1..=HIST_LEN`，不会为空。
    pub fn get_previous_color(&self) -> Color {
        *self.hist.last().expect("颜色历史恒非空")
    }

    // ---- 颜色调整（六个动作）----

    /// 对当前色执行一个颜色调整动作，返回是否真的变了。
    ///
    /// 模拟源码那条链：回调读 `app.brush.get_color_hsv()`、改一个分量、
    /// 再 `brush.set_color_hsv()`（`document.py:1595-1644`）；随后
    /// `BrushColorManager.__settings_changed_cb`（`brushcolor.py:39-48`）用
    /// `HSVColor(*brush.get_color_hsv())` 回填管理器。所以执行完，当前色的
    /// **类型变成 `HSVColor`**（这也是源码 `lib/color.py:13-18` 说的
    /// "只有调整器更新中央色时它的类型才跟着变"）。
    pub fn adjust(&mut self, act: ColorAction) -> bool {
        let hsv = self.color.get_hsv();
        let new = Color::Hsv(HsvColor::from_hsv(act.apply(hsv)));
        self.set_color(new)
    }

    /// 当前色 `[h, s, v]`（源码 `app.brush.get_color_hsv()` 的等价物）。
    pub fn color_hsv(&self) -> [f64; 3] {
        self.color.get_hsv()
    }

    // ---- ContextRestoreColor（`misc.context_restores_color`）----

    /// 当前开关状态（源码偏好的读取点：`document.py:1717`）。
    pub fn context_restores_color(&self) -> bool {
        self.context_restores_color
    }

    /// 设置开关（`document.py:1721-1724` 的 `context_toggle_color_cb` 写偏好）。
    pub fn set_context_restores_color(&mut self, value: bool) {
        self.context_restores_color = value;
    }

    /// 翻转开关（键盘 ` 键的动作 `ContextRestoreColor`，
    /// `resources.xml:381-386`），返回新状态。
    pub fn toggle_context_restores_color(&mut self) -> bool {
        self.context_restores_color = !self.context_restores_color;
        self.context_restores_color
    }

    /// 恢复笔刷槽位时的颜色处理 —— `document.context_cb` 的非 store 分支
    /// （`gui/document.py:1714-1719`）：
    ///
    /// ```python
    /// bm.select_brush(context)
    /// if self.app.preferences["misc.context_restores_color"]:
    ///     self.app.brushmodifier.restore_context_of_selected_brush()
    /// ```
    ///
    /// 而 `restore_context_of_selected_brush()`（`gui/brushmodifier.py`）做的是
    /// `c = unmodified_brushinfo.get_color_hsv(); app.brush.set_color_hsv(c)`
    /// —— 即"从槽位快照里把颜色取回来"，随后同样经
    /// `BrushColorManager` 的 settings-changed 回调落成 `HSVColor`。
    ///
    /// 返回是否恢复了颜色（关掉开关时**不**动当前色）。
    pub fn restore_brush_context(&mut self, saved_hsv: [f64; 3]) -> bool {
        if !self.context_restores_color {
            return false;
        }
        self.set_color(Color::Hsv(HsvColor::from_hsv(saved_hsv)));
        true
    }

    /// 存入笔刷槽位时要记下的颜色 —— `context.brushinfo = app.brush.clone()`
    /// （`document.py:1711`）里 `color_*` 三个基值，也就是 `get_color_hsv()`。
    pub fn brush_slot_color(&self) -> [f64; 3] {
        self.color.get_hsv()
    }
}
