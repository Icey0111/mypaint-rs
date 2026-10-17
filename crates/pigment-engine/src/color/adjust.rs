//! 颜色 / 笔刷调整动作 —— `gui/document.py` 那十个回调的语义。
//!
//! ## 先说清楚：`Bigger/Smaller/MoreOpaque/LessOpaque` **不是**颜色调整
//!
//! 任务书里把这四个当成"颜色的 v/s 线性/乘性调整"，源码不是这么写的：
//!
//! | 动作 | 源码 | 实际改的东西 |
//! |---|---|---|
//! | `Bigger` | `document.py:1571-1574` | `radius_logarithmic += 0.3` |
//! | `Smaller` | `document.py:1576-1579` | `radius_logarithmic -= 0.3` |
//! | `MoreOpaque` | `document.py:1581-1585` | `opaque *= 1.8` |
//! | `LessOpaque` | `document.py:1587-1590` | `opaque /= 1.8` |
//!
//! 它们改的是**笔刷设置**（`app.brush_adjustment[cname]`，即
//! `application.py:558-571` 为每个可见设置项建的 `Gtk.Adjustment`），
//! 标签也写着 "Increase/Decrease Brush Size"（`resources.xml:270-301`）。
//! 真正调颜色的是另外六个：`Brighter`/`Darker`/`Purer`/`Grayer`/
//! `IncreaseHue`/`DecreaseHue`（`document.py:1592-1644`，
//! 标签在 `resources.xml:302-355`，菜单位置是 Menu→Color→Adjust Color）。
//!
//! `GtkAdjustment` 会把值夹到 `[lower, upper]`，上下界来自设置元数据
//! （`s.min`/`s.max` → libmypaint 的 `brushsettings.json`）：
//! `radius_logarithmic ∈ [-2.0, 6.0]`、`opaque ∈ [0.0, 2.0]`
//! —— 所以 `MoreOpaque` 能把不透明度顶到 2.0，不是 1.0。

// ---------------------------------------------------------------------------
// 数值常量（逐条对应源码行）
// ---------------------------------------------------------------------------

/// 色相每步增量 —— `gui/document.py:1615` / `:1623` 的 `e = 0.015`。
pub const HUE_STEP: f64 = 0.015;
/// 明度 / 饱和度每步增量 —— `document.py:1596`(v)/`:1605`(v)/`:1631`(s)/`:1640`(s) 的 `0.08`。
pub const LEVEL_STEP: f64 = 0.08;
/// 明度 / 饱和度的下限 —— `document.py:1607` / `:1643` 的 `0.005`。
///
/// 源码注释（`:1606`、`:1641`）："stop a little higher than 0.0, to avoid
/// resetting hue to 0" —— 到 0 之后 HSV 的色相就没意义了。
pub const LEVEL_FLOOR: f64 = 0.005;
/// 明度 / 饱和度的上限 —— `document.py:1597` / `:1632` 的 `1.0`。
pub const LEVEL_CEIL: f64 = 1.0;
/// 笔刷半径（对数）每步增量 —— `document.py:1574` / `:1579` 的 `0.3`。
pub const BRUSH_RADIUS_STEP: f64 = 0.3;
/// 不透明度的乘 / 除系数 —— `document.py:1585` / `:1590` 的 `1.8`。
pub const BRUSH_OPAQUE_FACTOR: f64 = 1.8;
/// `radius_logarithmic` 的下界（libmypaint 设置元数据）。
pub const RADIUS_LOG_MIN: f64 = -2.0;
/// `radius_logarithmic` 的上界。
pub const RADIUS_LOG_MAX: f64 = 6.0;
/// `opaque` 的下界。
pub const OPAQUE_MIN: f64 = 0.0;
/// `opaque` 的上界（**注意是 2.0**，不是 1.0）。
pub const OPAQUE_MAX: f64 = 2.0;

/// `misc.context_restores_color` 的默认值（`gui/userconfig.py:93`）。
pub const CONTEXT_RESTORES_COLOR_DEFAULT: bool = true;

// ---------------------------------------------------------------------------
// 颜色调整（六个动作）
// ---------------------------------------------------------------------------

/// 菜单 Menu→Color→Adjust Color 里的六个颜色调整动作。
///
/// 名字就是 `resources.xml` 里的 GtkAction id，键位见 `resources.xml:309` 等。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorAction {
    /// `Brighter`：`v += 0.08`，上限 1.0（`document.py:1592-1599`）。
    Brighter,
    /// `Darker`：`v -= 0.08`，下限 0.005（`document.py:1601-1609`）。
    Darker,
    /// `IncreaseHue`：`h = (h + 0.015) % 1.0`（`document.py:1611-1617`）。
    IncreaseHue,
    /// `DecreaseHue`：`h = (h - 0.015) % 1.0`（`document.py:1619-1625`）。
    DecreaseHue,
    /// `Purer`：`s += 0.08`，上限 1.0（`document.py:1627-1634`）。
    Purer,
    /// `Grayer`：`s -= 0.08`，下限 0.005（`document.py:1636-1644`）。
    Grayer,
}

impl ColorAction {
    /// 六个动作的 GtkAction id（`gui/resources.xml` 里的 `id`，也是键表里的名字）。
    pub const ALL: [ColorAction; 6] = [
        ColorAction::Brighter,
        ColorAction::Darker,
        ColorAction::IncreaseHue,
        ColorAction::DecreaseHue,
        ColorAction::Purer,
        ColorAction::Grayer,
    ];

    /// 动作 id（`resources.xml` 的 `<object id="...">`）。
    pub fn action_name(self) -> &'static str {
        match self {
            ColorAction::Brighter => "Brighter",
            ColorAction::Darker => "Darker",
            ColorAction::IncreaseHue => "IncreaseHue",
            ColorAction::DecreaseHue => "DecreaseHue",
            ColorAction::Purer => "Purer",
            ColorAction::Grayer => "Grayer",
        }
    }

    /// 由动作 id 取动作；不是这六个就返回 `None`。
    pub fn from_action_name(name: &str) -> Option<ColorAction> {
        ColorAction::ALL
            .into_iter()
            .find(|a| a.action_name() == name)
    }

    /// 在 HSV 三元组上执行动作，返回新的 `(h, s, v)`。
    ///
    /// 逐行对应源码回调整体：它们都是
    /// `h, s, v = app.brush.get_color_hsv()` → 改一个分量 → `set_color_hsv((h,s,v))`。
    ///
    /// 三处容易漏的夹取规则（源码就是不对称的，照抄）：
    /// - `Brighter` 只夹上限、`Darker` 只夹下限（`0.005`）；
    /// - `Purer` 只夹上限、`Grayer` 只夹下限（`0.005`）；
    /// - 色相是**环形**的（`% 1.0`），不需要夹取。
    pub fn apply(self, hsv: [f64; 3]) -> [f64; 3] {
        let [h, s, v] = hsv;
        match self {
            // document.py:1595-1599
            ColorAction::Brighter => {
                let mut v = v + LEVEL_STEP;
                if v > LEVEL_CEIL {
                    v = LEVEL_CEIL;
                }
                [h, s, v]
            }
            // document.py:1604-1609
            ColorAction::Darker => {
                let mut v = v - LEVEL_STEP;
                // 停在 0.005：到 0 之后色相会被重置（:1606 的注释）
                if v < LEVEL_FLOOR {
                    v = LEVEL_FLOOR;
                }
                [h, s, v]
            }
            // document.py:1614-1617
            ColorAction::IncreaseHue => [(h + HUE_STEP).rem_euclid(1.0), s, v],
            // document.py:1622-1625
            ColorAction::DecreaseHue => [(h - HUE_STEP).rem_euclid(1.0), s, v],
            // document.py:1630-1634
            ColorAction::Purer => {
                let mut s = s + LEVEL_STEP;
                if s > LEVEL_CEIL {
                    s = LEVEL_CEIL;
                }
                [h, s, v]
            }
            // document.py:1639-1644
            ColorAction::Grayer => {
                let mut s = s - LEVEL_STEP;
                if s < LEVEL_FLOOR {
                    s = LEVEL_FLOOR;
                }
                [h, s, v]
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 笔刷设置的调整（四个动作）
// ---------------------------------------------------------------------------

/// `Bigger` / `Smaller` / `MoreOpaque` / `LessOpaque` —— 改**笔刷设置**的动作。
///
/// 见模块文档的表格：前两个动 `radius_logarithmic`，后两个动 `opaque`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushStep {
    /// `Bigger`：`radius_logarithmic += 0.3`（`document.py:1571-1574`）。
    Bigger,
    /// `Smaller`：`radius_logarithmic -= 0.3`（`document.py:1576-1579`）。
    Smaller,
    /// `MoreOpaque`：`opaque *= 1.8`（`document.py:1581-1585`）。
    MoreOpaque,
    /// `LessOpaque`：`opaque /= 1.8`（`document.py:1587-1590`）。
    LessOpaque,
}

impl BrushStep {
    /// 四个动作的 GtkAction id。
    pub const ALL: [BrushStep; 4] = [
        BrushStep::Bigger,
        BrushStep::Smaller,
        BrushStep::MoreOpaque,
        BrushStep::LessOpaque,
    ];

    /// 动作 id。
    pub fn action_name(self) -> &'static str {
        match self {
            BrushStep::Bigger => "Bigger",
            BrushStep::Smaller => "Smaller",
            BrushStep::MoreOpaque => "MoreOpaque",
            BrushStep::LessOpaque => "LessOpaque",
        }
    }

    /// 由动作 id 取动作。
    pub fn from_action_name(name: &str) -> Option<BrushStep> {
        BrushStep::ALL
            .into_iter()
            .find(|a| a.action_name() == name)
    }

    /// 这个动作作用在哪个笔刷设置上（内部名 = `BrushSetting::cname()`）。
    pub fn cname(self) -> &'static str {
        match self {
            BrushStep::Bigger | BrushStep::Smaller => "radius_logarithmic",
            BrushStep::MoreOpaque | BrushStep::LessOpaque => "opaque",
        }
    }

    /// 该设置的上下界（libmypaint 元数据；`GtkAdjustment` 的 lower/upper）。
    pub fn bounds(self) -> (f64, f64) {
        match self {
            BrushStep::Bigger | BrushStep::Smaller => (RADIUS_LOG_MIN, RADIUS_LOG_MAX),
            BrushStep::MoreOpaque | BrushStep::LessOpaque => (OPAQUE_MIN, OPAQUE_MAX),
        }
    }

    /// 在给定的基值上执行一次动作，返回新值（已按 adjustment 的上下界夹取）。
    ///
    /// 对应源码 `adj.set_value(adj.get_value() ± ...)`：`GtkAdjustment.set_value`
    /// 会把超界的值夹回 `[lower, upper]`（`application.py:563-570` 建 adjustment
    /// 时用的就是 `s.min`/`s.max`）。
    pub fn apply(self, value: f64) -> f64 {
        let (lo, hi) = self.bounds();
        let raw = match self {
            BrushStep::Bigger => value + BRUSH_RADIUS_STEP,
            BrushStep::Smaller => value - BRUSH_RADIUS_STEP,
            BrushStep::MoreOpaque => value * BRUSH_OPAQUE_FACTOR,
            BrushStep::LessOpaque => value / BRUSH_OPAQUE_FACTOR,
        };
        raw.clamp(lo, hi)
    }
}

// ---------------------------------------------------------------------------
// 颜色调整后中央色的类型
// ---------------------------------------------------------------------------
//
// 注意类型规则：颜色调整后当前色变成 `HSVColor`。依据是源码的两段 ——
// 回调读 `app.brush.get_color_hsv()`、写 `brush.set_color_hsv()`
// （`document.py:1595-1599`），而 `BrushColorManager.__settings_changed_cb`
// （`gui/brushcolor.py:39-48`）看到 `color_h/s/v` 变化后用
// `HSVColor(*brush.get_color_hsv())` 回填管理器 —— 于是"共享颜色的类型跟着
// 调整器所在的色彩空间变"（`lib/color.py:13-18`）。具体落地在
// [`super::ColorManager::adjust`]。

