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
//! ## 与源码的对应关系
//!
//! - **偏好持久化（B6-1）**：源码从 `prefs["colors.current"]` /
//!   `prefs["colors.history"]` 读、写回（`adjbases.py:121-131`、`:216`、
//!   `:258-263`），色轮类型见 `:133-136`/`:307`，调色板见 `:139-147`/`:354-357`。
//!   这里对应 [`ColorManager::from_prefs`]（启动读）与
//!   [`ColorManager::store_prefs`]（保存时整份写回）。
//! - **adjuster 通知**：`color_updated` / `color_history_updated`
//!   （`lib/observable.py` 的 `@event`）是给 GUI 控件用的；egui 是立即模式，
//!   控件每帧读管理器即可（`pigment-app` 的 `shell/` 就是这么做的）。
//! - **拾色器光标** `_picker_cursor`（`adjbases.py:114-116`）：Gdk 对象，
//!   归 GUI 层。
//! - **色轮色相畸变表** `_hue_distorts` / `distort_hue()`（`:80-95`、`:289-345`）：
//!   B4-4b 已接上（[`ColorManager::wheel_type`] / [`ColorManager::distort_hue`]），
//!   表的数值与插值在 `color::wheels` 里逐行对照并有真源码向量门禁。
//!   `set_wheel_type()` 里"重画所有色轮背景"那一步（`:308-310` 的
//!   `adj.clear_background()`）是控件缓存，egui 每帧重画，不需要。

use super::adjust::{ColorAction, CONTEXT_RESTORES_COLOR_DEFAULT};
use super::model::{colors_match, Color, HsvColor, RgbColor, UIColor};
use super::palette::{Palette, SimplePaletteDict};
use super::wheels::{distort_hue, undistort_hue, HueRange, WheelType};
use crate::prefs::Prefs;

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
/// 偏好键：色轮类型（`adjbases.py:47` `PREFS_KEY_WHEEL_TYPE`）。
///
/// 值见 [`WheelType`]（`"rgb"` / `"ryb"` / `"rygb"`，`adjbases.py:80-95`）。
pub const PREFS_KEY_WHEEL_TYPE: &str = super::wheels::PREFS_KEY_WHEEL_TYPE;
/// 偏好键：当前调色板 —— `adjbases.py:48` 的 `PREFS_PALETTE_DICT_KEY`。
///
/// 值是调色板的"简单字典"（`palette.py:831-859`，见 [`SimplePaletteDict`]），
/// 不是 `.gpl` 文本。
pub const PREFS_KEY_PALETTE: &str = "colors.palette";
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
    /// 色轮类型（源码 `_hue_distorts` 的表索引 + `prefs[PREFS_KEY_WHEEL_TYPE]`，
    /// `adjbases.py:118` / `:133-136` / `:291-315`）。
    ///
    /// 源码把 `_hue_distorts` 存成表本身（`None` 或区间序列）；这里存类型标签，
    /// 表由 `WheelType::ranges()` 给出 —— 两者等价，但类型标签还能写回偏好。
    wheel_type: WheelType,
    /// 当前工作调色板（源码 `_palette`，`adjbases.py:111`）。
    ///
    /// 源码在 `__init__` 里从偏好读、读不到就载入
    /// `<datapath>/palettes/MyPaint_Default.gpl`（`adjbases.py:139-147`）。
    /// 偏好系统属 B6，这里只走"载入默认调色板"那一支
    /// （[`Palette::mypaint_default`]，内容内嵌自同一个文件）。
    palette: Palette,
}

impl Default for ColorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorManager {
    /// 按源码的启动路径初始化：历史 = `_DEFAULT_HIST`，当前色 = 历史末项，
    /// 调色板 = 默认 `.gpl`。
    ///
    /// 对应 `adjbases.py:121-147` 的"没有偏好"那一档；有偏好时用
    /// [`ColorManager::from_prefs`]（B6-1 起应用启动走那条）。
    pub fn new() -> Self {
        let hist = hist_from_hex(&DEFAULT_HIST_HEX);
        // hist_hex[-1]
        let color = *hist.last().expect("_DEFAULT_HIST 非空");
        Self {
            color,
            hist,
            context_restores_color: CONTEXT_RESTORES_COLOR_DEFAULT,
            // `adjbases.py:134-136`：`prefs.get(PREFS_KEY_WHEEL_TYPE, "rgb")`
            // 以及 `_HUE_DISTORTION_TABLES[wheel_type]`。没有偏好时走默认那一支。
            wheel_type: WheelType::DEFAULT,
            // `adjbases.py:139-147`：没有偏好就加载默认调色板文件
            palette: Palette::mypaint_default(),
        }
    }

    /// 从用户偏好初始化 —— `ColorManager.__init__` 的 `adjbases.py:121-147`。
    ///
    /// 逐段对应：
    /// - `:122-125` 历史 = `_DEFAULT_HIST + prefs[colors.history]`，再裁到 5；
    /// - `:128-131` 当前色 = `prefs[colors.current]`，缺失时取历史**末项**；
    /// - `:133-136` 色轮类型 = `prefs[colors.wheels.type]`（缺失 `"rgb"`）；
    /// - `:139-147` 调色板 = `prefs[colors.palette]` 的简单字典，
    ///   缺失时载入 `<datapath>/palettes/MyPaint_Default.gpl`（内嵌版）；
    /// - `:93`（`userconfig.py`）`misc.context_restores_color`。
    ///
    /// 源码读不到期望类型时会抛异常（`new_from_hex_str` 对非串、
    /// `_HUE_DISTORTION_TABLES[未知名字]` 的 `KeyError`）；这里按最保守的
    /// "当作没有偏好"处理（跳过坏历史项 / 未知色轮名退回 `rgb` /
    /// 坏调色板退回默认 `.gpl`）。
    pub fn from_prefs(prefs: &Prefs) -> Self {
        // `:122-125`
        let mut hist = hist_from_hex(&DEFAULT_HIST_HEX);
        if let Some(arr) = prefs.get_array(PREFS_KEY_COLOR_HISTORY) {
            for v in arr {
                if let Some(s) = v.as_str() {
                    hist.push(Color::Rgb(RgbColor::new_from_hex_str(s)));
                }
            }
        }
        while hist.len() > HIST_LEN {
            hist.remove(0);
        }
        // `:128-131`
        let color = match prefs.get_str(PREFS_KEY_CURRENT_COLOR) {
            Some(s) => Color::Rgb(RgbColor::new_from_hex_str(s)),
            None => *hist.last().expect("_DEFAULT_HIST 非空"),
        };
        // `:133-136`
        let wheel_type =
            WheelType::from_name(prefs.str_or(PREFS_KEY_WHEEL_TYPE, WheelType::DEFAULT.name()))
                .unwrap_or(WheelType::DEFAULT);
        // `:139-147`
        let palette = match prefs.get(PREFS_KEY_PALETTE) {
            Some(v) => SimplePaletteDict::from_json(v)
                .map(|simple| Palette::from_simple_dict(&simple))
                .unwrap_or_else(Palette::mypaint_default),
            None => Palette::mypaint_default(),
        };
        Self {
            color,
            hist,
            // `misc.context_restores_color`（`userconfig.py:93` 默认 True）
            context_restores_color: prefs.bool_or(
                PREFS_KEY_CONTEXT_RESTORES_COLOR,
                CONTEXT_RESTORES_COLOR_DEFAULT,
            ),
            wheel_type,
            palette,
        }
    }

    /// 把当前状态写回偏好 —— 源码"改动即写"那几处的收拢版。
    ///
    /// | 写到哪 | 源码 |
    /// |---|---|
    /// | `colors.current` | `set_color()`，`adjbases.py:216` |
    /// | `colors.history` | `push_history()`，`:258-263` |
    /// | `colors.wheels.type` | `set_wheel_type()`，`:307` |
    /// | `colors.palette` | `_palette_changed_cb()`，`:354-357` |
    /// | `misc.context_restores_color` | `document.py:1721-1724` |
    ///
    /// **时机差异（记账）**：源码在每次改动时立刻写进共享 dict；这里由
    /// 应用层在保存配置（`save_gui_config`，`application.py:624-628`）时
    /// 统一调用一次。落盘内容一致，但崩溃前不会保住最后一次颜色/调色板改动。
    pub fn store_prefs(&self, prefs: &mut Prefs) {
        prefs.set_str(PREFS_KEY_CURRENT_COLOR, self.color.to_hex_str());
        let hist = self.history_hex();
        prefs.set_str_array(PREFS_KEY_COLOR_HISTORY, hist.iter().map(String::as_str));
        prefs.set_str(PREFS_KEY_WHEEL_TYPE, self.wheel_type_name());
        prefs.set(PREFS_KEY_PALETTE, self.palette.to_simple_dict().to_json());
        prefs.set_bool(
            PREFS_KEY_CONTEXT_RESTORES_COLOR,
            self.context_restores_color,
        );
    }

    // ---- 当前色（`adjbases.py:202-226`）----

    /// `set_color()`（`adjbases.py:204-220`）：颜色**没变就不动**（`:213-214`
    /// 的 `if color == self._color: return`），否则存一份拷贝。
    ///
    /// 换色之后紧接着 `self._palette.match_color(color)`（`:219`）——
    /// 这是"调色板上高亮当前色"的唯一驱动：调色板页不自己算命中。
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
        // `adjbases.py:219`：`self._palette.match_color(color)`
        let c = self.color.copied();
        self.palette.match_color(&c, false, None);
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

    // ---- 调色板（`adjbases.py:347-357`）----

    /// 当前工作调色板 —— `ColorManager.palette` 属性（`adjbases.py:349-352`）。
    ///
    /// 源码把它挂在管理器上（而不是各 adjuster 自己持一份），所以调色板页与
    /// `PaletteNext`/`PalettePrev`/`PaletteAddCurrentColor` 三个动作看到的是
    /// **同一个**调色板。
    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// 可变引用（调色板页的编辑操作要用）。
    pub fn palette_mut(&mut self) -> &mut Palette {
        &mut self.palette
    }

    // ---- 色轮类型与色相畸变（`adjbases.py:289-345`）----

    /// 当前色轮类型 —— 源码 `_hue_distorts` 指向的那张表 + 偏好
    /// `colors.wheels.type`（`adjbases.py:133-136` / `:312-315`）。
    pub fn wheel_type(&self) -> WheelType {
        self.wheel_type
    }

    /// 当前色轮类型的名字 —— `get_wheel_type()`（`adjbases.py:312-315`）
    /// 返回的就是这个串（`"rgb"` / `"ryb"` / `"rygb"`）。
    pub fn wheel_type_name(&self) -> &'static str {
        self.wheel_type.name()
    }

    /// 切换色轮类型 —— `set_wheel_type()`（`adjbases.py:291-310`）。
    ///
    /// 源码流程：老类型 == 新类型就直接返回（`:304-305`）；否则换表
    /// （`:306`）、写偏好（`:307`）、让所有色轮清缓存（`:308-310`）。
    /// 最后一步是控件缓存，egui 每帧重画，不需要。
    ///
    /// 返回是否真的变了（应用层据此决定要不要重绘/记偏好）。
    pub fn set_wheel_type(&mut self, wheel_type: WheelType) -> bool {
        if wheel_type == self.wheel_type {
            return false;
        }
        self.wheel_type = wheel_type;
        true
    }

    /// 按名字切换 —— `set_wheel_type(typename)` 的完整语义
    /// （`adjbases.py:291-310`）：**未知名字退回默认值**（`:302-303`）。
    pub fn set_wheel_type_by_name(&mut self, name: &str) -> bool {
        let wheel_type = WheelType::from_name(name).unwrap_or(WheelType::DEFAULT);
        self.set_wheel_type(wheel_type)
    }

    /// 当前色轮类型的重映射表 —— `self._hue_distorts`（`adjbases.py:118`）。
    ///
    /// `None` = `"rgb"` 轮，`distort_hue()`/`undistort_hue()` 都恒等。
    pub fn hue_ranges(&self) -> Option<&'static [HueRange]> {
        self.wheel_type.ranges()
    }

    /// 色相畸变 —— `ColorManager.distort_hue()`（`adjbases.py:317-330`）。
    ///
    /// 色轮画标记 / 画扇区时用：颜色里的 RGB 轮角度 → 当前轮类型角度。
    pub fn distort_hue(&self, h: f64) -> f64 {
        distort_hue(h, self.hue_ranges())
    }

    /// 色相逆畸变 —— `ColorManager.undistort_hue()`（`adjbases.py:332-345`）。
    ///
    /// 色轮取色时用：点到（当前轮类型的）角度 → 存进颜色的 RGB 轮角度。
    pub fn undistort_hue(&self, h: f64) -> f64 {
        undistort_hue(h, self.hue_ranges())
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
