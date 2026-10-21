//! 兼容模式 —— `gui/compatibility.py` + `gui/compatconfig.py`。
//!
//! MyPaint 2 引入了"兼容模式"：1.x 模式按旧版语义工作（EOTF 1.0、
//! 新笔刷默认不开颜料混色、新层默认 Normal），2.x 是现行语义（EOTF 2.2、
//! 颜料混色默认开、新层默认 Pigment）。
//!
//! # 源码的三个决定点
//!
//! 1. **打开文件**（`compatibility.py:412-434` 的 `ora_compat_handler`）：
//!    - `stack.xml` 有 `mypaint:eotf` 且值为 1.0 → 1.x，否则 2.x；
//!    - 没有该属性 → 按偏好 `compat_behavior_when_unknown`
//!      （`always-1.x` / `always-2.x` / `unless-pigment-layer-1.x`）决定，
//!      最后一档看 `has_pigment_layers(root_elem)`（`:70-84`）；
//!    - 决定后 `set_compat_mode`（`:437-470`）切换 EOTF、新层默认类型、
//!      笔刷默认颜料开关。
//! 2. **新建文档**：用偏好 `default_compatibility_mode`。
//! 3. **偏好页**：两套 `compability_settings`（源码拼写照抄）分别有
//!    `pigment_on_by_default` 与 `pigment_layer_is_default`。
//!
//! # 本移植的范围
//!
//! 已实现：模式判定、EOTF 标签读写、新层默认类型、笔刷默认颜料、
//! 文件兼容警告的级别与偏好开关。
//!
//! 记账：`lib.eotf` 是全局可变值，而本实现的合成/转换用的是
//! `pigment_surface::color::EOTF = 2.2` 常量；1.x 的 EOTF=1.0 **只写进
//! 文件标签**，颜色转换仍按 2.2（见 `color.rs` 的模块文档）。真正的
//! 1.x 旧混合数学（`lib/modes.py` 的 legacy 映射）也没有单独实现。

use serde_json::{json, Value};

use crate::prefs::Prefs;
use pigment_surface::combine::CombineMode;
use pigment_surface::layer::{LayerMode, LayerStack};

/// 1.x 模式名（`gui/compatconfig.py:32` 的 `C1X`）。
pub const C1X: &str = "1.x";
/// 2.x 模式名（`gui/compatconfig.py:33` 的 `C2X`）。
pub const C2X: &str = "2.x";

/// EOTF 1.x —— `lib/eotf.py` 的 `set_eotf(1.0)`（`compatibility.py:458-460`）。
pub const EOTF_1X: f32 = 1.0;
/// EOTF 2.x —— `DEFAULT_EOTF = 2.2`（`lib/eotf.py:16`）。
pub const EOTF_2X: f32 = 2.2;

/// 兼容模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompatMode {
    /// 1.x（旧版语义）。
    C1X,
    /// 2.x（现行语义）。
    C2X,
}

impl CompatMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::C1X => C1X,
            Self::C2X => C2X,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            C1X => Some(Self::C1X),
            C2X => Some(Self::C2X),
            _ => None,
        }
    }

    /// 该模式的 EOTF 标签 —— `set_compat_mode`（`compatibility.py:458-463`）。
    pub fn eotf(self) -> f32 {
        match self {
            Self::C1X => EOTF_1X,
            Self::C2X => EOTF_2X,
        }
    }
}

/// 一个模式下的两条设置 —— `COMPAT_SETTINGS[mode]`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeSettings {
    /// `pigment_on_by_default`：新笔刷没写 `paint_mode` 时是否默认开颜料。
    pub pigment_on_by_default: bool,
    /// `pigment_layer_is_default`：新建绘画层的默认混合模式是否 Pigment。
    pub pigment_layer_is_default: bool,
}

impl ModeSettings {
    pub const fn new(pigment_on_by_default: bool, pigment_layer_is_default: bool) -> Self {
        Self {
            pigment_on_by_default,
            pigment_layer_is_default,
        }
    }
}

impl Default for ModeSettings {
    /// 出厂值（`gui/compatconfig.py:44-48`）：
    /// 1.x 两项都 false，2.x 两项都 true。
    fn default() -> Self {
        Self::new(true, true)
    }
}

/// 两套模式设置 —— 偏好 `compability_settings`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompatSettings {
    pub one_x: ModeSettings,
    pub two_x: ModeSettings,
}

impl Default for CompatSettings {
    fn default() -> Self {
        Self {
            one_x: ModeSettings::new(false, false),
            two_x: ModeSettings::new(true, true),
        }
    }
}

impl CompatSettings {
    pub fn mode(&self, mode: CompatMode) -> ModeSettings {
        match mode {
            CompatMode::C1X => self.one_x,
            CompatMode::C2X => self.two_x,
        }
    }

    pub fn mode_mut(&mut self, mode: CompatMode) -> &mut ModeSettings {
        match mode {
            CompatMode::C1X => &mut self.one_x,
            CompatMode::C2X => &mut self.two_x,
        }
    }

    /// 从偏好读嵌套对象；缺失/类型不对用出厂值。
    pub fn from_prefs(prefs: &Prefs) -> Self {
        let mut out = Self::default();
        let Some(obj) = prefs.get_object("compability_settings") else {
            return out;
        };
        for (name, target) in [("1.x", &mut out.one_x), ("2.x", &mut out.two_x)] {
            let Some(m) = obj.get(name).and_then(|v| v.as_object()) else {
                continue;
            };
            if let Some(v) = m.get("pigment_on_by_default").and_then(|v| v.as_bool()) {
                target.pigment_on_by_default = v;
            }
            if let Some(v) = m.get("pigment_layer_is_default").and_then(|v| v.as_bool()) {
                target.pigment_layer_is_default = v;
            }
        }
        out
    }

    /// 写回偏好 —— `CompatibilityPreferences._update_prefs`
    /// （`compatibility.py:394-396` 的 `prefs[COMPAT_SETTINGS][mode].update`）。
    pub fn store(&self, prefs: &mut Prefs) {
        let value = json!({
            "1.x": {
                "pigment_on_by_default": self.one_x.pigment_on_by_default,
                "pigment_layer_is_default": self.one_x.pigment_layer_is_default,
            },
            "2.x": {
                "pigment_on_by_default": self.two_x.pigment_on_by_default,
                "pigment_layer_is_default": self.two_x.pigment_layer_is_default,
            },
        });
        prefs.set("compability_settings", value);
    }
}

/// 文件没有 EOTF 时的行为 —— 偏好 `compat_behavior_when_unknown`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileBehavior {
    /// `always-1.x`
    Always1x,
    /// `always-2.x`
    Always2x,
    /// `unless-pigment-layer-1.x`（默认）
    UnlessPigmentLayer1x,
}

impl FileBehavior {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always-1.x" => Self::Always1x,
            "always-2.x" => Self::Always2x,
            _ => Self::UnlessPigmentLayer1x,
        }
    }

    /// 选模式 —— `CompatFileBehavior.get_compat_mode`（`compatibility.py:248-269`）。
    pub fn mode_for(self, has_pigment: bool, _default: CompatMode) -> CompatMode {
        match self {
            Self::Always1x => CompatMode::C1X,
            Self::Always2x => CompatMode::C2X,
            Self::UnlessPigmentLayer1x => {
                if has_pigment {
                    CompatMode::C2X
                } else {
                    CompatMode::C1X
                }
            }
        }
    }
}

/// 打开一个 ORA 时决定兼容模式 —— `ora_compat_handler`（`compatibility.py:412-434`）。
///
/// `eotf` 是文件里 `mypaint:eotf` 的值；`has_pigment` 是文件图层栈里是否
/// 有 Pigment 模式层。有 EOTF 时以它为准（1.0 → 1.x，其它 → 2.x），
/// 没有才看用户偏好。
pub fn mode_for_file(
    eotf: Option<f32>,
    has_pigment: bool,
    behavior: FileBehavior,
    default: CompatMode,
) -> CompatMode {
    match eotf {
        Some(v) if v == EOTF_1X => CompatMode::C1X,
        Some(_) => CompatMode::C2X,
        None => behavior.mode_for(has_pigment, default),
    }
}

/// 图层栈里是否有颜料（SpectralWGM）层 —— `has_pigment_layers`
/// （`compatibility.py:70-84` 的递归版本）。
pub fn layers_have_pigment(layers: &LayerStack) -> bool {
    layers
        .walk()
        .iter()
        .any(|(_, l)| l.mode == LayerMode::Standard(CombineMode::SpectralWGM))
}

/// 解析偏好里的模式名（`default_compatibility_mode`），非法回落 2.x
/// （`compatibility.py:449-451` 的 "Unknown compatibility mode" 分支）。
pub fn default_mode(prefs: &Prefs) -> CompatMode {
    CompatMode::from_str(prefs.str_or("default_compatibility_mode", C2X)).unwrap_or(CompatMode::C2X)
}

/// 把 `compability_settings` 的 JSON 值原样读出来（测试/诊断用）。
pub fn settings_json(prefs: &Prefs) -> Option<&Value> {
    prefs.get("compability_settings")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prefs(tag: &str) -> Prefs {
        let mut p: PathBuf = std::env::temp_dir();
        p.push(format!("pigment-compat-{}-{tag}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        Prefs::load(p)
    }

    #[test]
    fn default_settings_match_compatconfig() {
        let p = prefs("defaults");
        let s = CompatSettings::from_prefs(&p);
        assert_eq!(s.one_x, ModeSettings::new(false, false));
        assert_eq!(s.two_x, ModeSettings::new(true, true));
        assert_eq!(default_mode(&p), CompatMode::C2X);
        assert_eq!(CompatMode::C1X.eotf(), 1.0);
        assert_eq!(CompatMode::C2X.eotf(), 2.2);
        let _ = std::fs::remove_file(p.path());
    }

    #[test]
    fn settings_roundtrip_through_the_prefs_object() {
        let mut p = prefs("roundtrip");
        let mut s = CompatSettings::default();
        s.one_x.pigment_on_by_default = true;
        s.two_x.pigment_layer_is_default = false;
        s.store(&mut p);
        let back = CompatSettings::from_prefs(&p);
        assert_eq!(back, s);
        assert!(settings_json(&p).is_some());
        let _ = std::fs::remove_file(p.path());
    }

    #[test]
    fn file_mode_follows_eotf_then_behavior() {
        // 有 EOTF：1.0 → 1.x，其它 → 2.x（即使偏好说 1.x）
        assert_eq!(
            mode_for_file(Some(1.0), false, FileBehavior::Always2x, CompatMode::C2X),
            CompatMode::C1X
        );
        assert_eq!(
            mode_for_file(Some(2.2), false, FileBehavior::Always1x, CompatMode::C1X),
            CompatMode::C2X
        );
        // 没有 EOTF：按偏好
        assert_eq!(
            mode_for_file(None, false, FileBehavior::Always1x, CompatMode::C2X),
            CompatMode::C1X
        );
        assert_eq!(
            mode_for_file(None, true, FileBehavior::Always2x, CompatMode::C1X),
            CompatMode::C2X
        );
        // unless-pigment-layer：有颜料层 → 2.x，没有 → 1.x
        let b = FileBehavior::UnlessPigmentLayer1x;
        assert_eq!(
            mode_for_file(None, true, b, CompatMode::C2X),
            CompatMode::C2X
        );
        assert_eq!(
            mode_for_file(None, false, b, CompatMode::C2X),
            CompatMode::C1X
        );
        // 非法行为串回落 unless-pigment-layer（源码的未知值分支用默认，
        // 但兼容配置只有三档；这里保守取默认档）
        assert_eq!(
            FileBehavior::from_str("nope"),
            FileBehavior::UnlessPigmentLayer1x
        );
    }

    #[test]
    fn pigment_layer_detection_walks_groups() {
        use pigment_surface::layer::Layer;
        let mut st = LayerStack::new();
        // 出厂新层就是 SpectralWGM（`Layer::default`），先改成 Normal 再判
        st.layer_mut(0)
            .set_mode(LayerMode::Standard(CombineMode::Normal));
        assert!(!layers_have_pigment(&st));
        let mut l = Layer::new();
        l.set_mode(LayerMode::Standard(CombineMode::SpectralWGM));
        st.insert(0, l);
        assert!(layers_have_pigment(&st));
    }
}
