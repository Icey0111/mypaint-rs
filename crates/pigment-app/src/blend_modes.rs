//! 笔刷/填充的混合模式互斥状态 —— `gui/blendmodehandler.py` 的纯逻辑移植。
//!
//! 源码 `BlendModes`（`blendmodehandler.py:38-111`）维护四个互斥开关：
//! `eraser` / `lock_alpha` / `colorize` / `normal`，并保留一个**激活历史栈**：
//!
//! - 激活非 Normal：先从历史中移除它，再压栈尾（`:82-88`）；
//! - 激活 Normal：清空历史（`:83-85`）；
//! - 取消激活当前模式：从栈顶弹出，跳过刚被取消的那个，激活剩下的栈顶；
//!   栈空则回到 Normal（`:90-96`）。
//!
//! 这解释了源码里“先开橡皮、再开锁 alpha、然后取消锁 alpha 会回到橡皮”的
//! 行为。工具模式各自持有自己的 `BlendModes` 实例
//! （`blendmodehandler.py:38-56`），本模块只提供状态机；按工具切换实例的
//! 逻辑在 `PigmentApp` 里。

use pigment_engine::hokusai::SettingValue;

/// `BrushModifier.set_override_setting` 的单个 setting 语义
/// （`brushmodifier.py:50-73`）。
///
/// - 目标状态已满足（开：base > 0.9；关：base < 0.1）→ `None`，保留现值
///   （包括输入映射）；
/// - 否则从 `unmodified`（笔刷原始设置）取同方向的设置（保留曲线），
///   若原始设置也不符合方向，则强制 1.0/0.0 **并清空输入映射** ——
///   源码常量 `MODE_FORCED_ON/OFF_SETTINGS = [1.0/0.0, {}]`。
///
/// 返回 `None` 表示调用方不需要改这个 setting。
pub(crate) fn override_setting_value(
    current: &SettingValue,
    unmodified: &SettingValue,
    on: bool,
) -> Option<SettingValue> {
    let already = if on {
        current.base_value > 0.9
    } else {
        current.base_value < 0.1
    };
    if already {
        return None;
    }
    let source_ok = if on {
        unmodified.base_value > 0.9
    } else {
        unmodified.base_value < 0.1
    };
    if source_ok {
        Some(unmodified.clone())
    } else {
        Some(SettingValue::constant(if on { 1.0 } else { 0.0 }))
    }
}

/// 四个互斥的笔刷混合模式。
///
/// 数值与 `blendmodehandler.py:53-56` 的 `NORMAL/ERASE/LOCK_ALPHA/COLORIZE`
/// 一致；`FillBlendMode` 也用同一组数值（`fill.py` 的 `mode_type`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum BrushBlendMode {
    Normal,
    Eraser,
    LockAlpha,
    Colorize,
}

impl BrushBlendMode {
    pub(crate) const ALL: [BrushBlendMode; 4] = [
        BrushBlendMode::Normal,
        BrushBlendMode::Eraser,
        BrushBlendMode::LockAlpha,
        BrushBlendMode::Colorize,
    ];

    /// `BlendMode.name`（`blendmodehandler.py:59-65`），也是动作表里的键。
    #[allow(dead_code)] // 源码 API 的镜像；运行时用 action_id/setting_name
    pub(crate) fn name(self) -> &'static str {
        match self {
            BrushBlendMode::Normal => "normal_mode",
            BrushBlendMode::Eraser => "eraser_mode",
            BrushBlendMode::LockAlpha => "lock_alpha_mode",
            BrushBlendMode::Colorize => "colorize_mode",
        }
    }

    /// `BlendMode.setting_name`（`blendmodehandler.py:59-65`）；Normal 没有。
    pub(crate) fn setting_name(self) -> Option<&'static str> {
        match self {
            BrushBlendMode::Normal => None,
            BrushBlendMode::Eraser => Some("eraser"),
            BrushBlendMode::LockAlpha => Some("lock_alpha"),
            BrushBlendMode::Colorize => Some("colorize"),
        }
    }

    pub(crate) fn action_id(self) -> &'static str {
        match self {
            BrushBlendMode::Normal => "BlendModeNormal",
            BrushBlendMode::Eraser => "BlendModeEraser",
            BrushBlendMode::LockAlpha => "BlendModeLockAlpha",
            BrushBlendMode::Colorize => "BlendModeColorize",
        }
    }

    pub(crate) fn from_action_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.action_id() == id)
    }

    /// `fill.FillBlendMode` 的数值（`fill.py` 的 `mode_type`）。
    pub(crate) fn index(self) -> u8 {
        match self {
            BrushBlendMode::Normal => 0,
            BrushBlendMode::Eraser => 1,
            BrushBlendMode::LockAlpha => 2,
            BrushBlendMode::Colorize => 3,
        }
    }

    pub(crate) fn from_index(i: u8) -> Self {
        match i {
            1 => BrushBlendMode::Eraser,
            2 => BrushBlendMode::LockAlpha,
            3 => BrushBlendMode::Colorize,
            _ => BrushBlendMode::Normal,
        }
    }
}

/// 一份 `BlendModes` 状态（`blendmodehandler.py:38-111`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlendModes {
    active: BrushBlendMode,
    /// `history`：只在非 Normal 模式之间使用。
    history: Vec<BrushBlendMode>,
}

impl Default for BlendModes {
    fn default() -> Self {
        Self {
            active: BrushBlendMode::Normal,
            history: Vec::new(),
        }
    }
}

impl BlendModes {
    pub(crate) fn active(&self) -> BrushBlendMode {
        self.active
    }

    /// 把某个模式设为唯一激活项。返回 active 是否真的变了。
    ///
    /// 对应 `BlendMode.active` setter + `_update()`（`:21-30`、`:98-106`）。
    /// 重复激活同一个模式不改变历史（源码 setter 在值相同时不派发 `changed`）。
    pub(crate) fn activate(&mut self, mode: BrushBlendMode) -> bool {
        if mode == self.active {
            return false;
        }
        let old = self.active;
        // 源码 `_update` 无条件先把旧模式的 `_active` 置 false，再处理新模式；
        // 对新模式调用 `_push_history`。
        self.active = mode;
        if mode == BrushBlendMode::Normal {
            self.history.clear();
        } else {
            self.history.retain(|m| *m != mode);
            self.history.push(mode);
        }
        old != self.active
    }

    /// 取消激活某个模式（源码 `mode.active = False` 的路径）。
    #[allow(dead_code)] // 保留源码状态机的完整入口；当前动作只走 activate
    ///
    /// 如果它不是当前模式，只从历史里移除；如果是当前模式，则按源码
    /// `_pop_history` 弹栈：跳过被取消的模式，激活新的栈顶，栈空回 Normal。
    pub(crate) fn deactivate(&mut self, mode: BrushBlendMode) -> bool {
        if mode != self.active {
            let old_len = self.history.len();
            self.history.retain(|m| *m != mode);
            return self.history.len() != old_len;
        }
        // `_pop_history(removed)`（`:90-96`）。
        while let Some(candidate) = self.history.pop() {
            if candidate != mode {
                self.active = candidate;
                return true;
            }
        }
        self.active = BrushBlendMode::Normal;
        true
    }

    /// 从笔刷的三个布尔 setting 推出应激活的模式。
    ///
    /// 顺序照 `brushmodifier.py:131-137`：eraser → lock_alpha → colorize 依次
    /// 覆盖，最后匹配到的获胜；都没有则 Normal。
    pub(crate) fn activate_from_settings(eraser: bool, lock_alpha: bool, colorize: bool) -> Self {
        let mut mode = BrushBlendMode::Normal;
        if eraser {
            mode = BrushBlendMode::Eraser;
        }
        if lock_alpha {
            mode = BrushBlendMode::LockAlpha;
        }
        if colorize {
            mode = BrushBlendMode::Colorize;
        }
        Self {
            active: mode,
            history: if mode == BrushBlendMode::Normal {
                Vec::new()
            } else {
                vec![mode]
            },
        }
    }

    /// 当前模式投影成三个布尔值（顺序 eraser/lock_alpha/colorize）。
    pub(crate) fn flags(&self) -> (bool, bool, bool) {
        (
            self.active == BrushBlendMode::Eraser,
            self.active == BrushBlendMode::LockAlpha,
            self.active == BrushBlendMode::Colorize,
        )
    }

    /// 历史栈（测试/诊断用）。
    #[allow(dead_code)]
    pub(crate) fn history(&self) -> &[BrushBlendMode] {
        &self.history
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BrushBlendMode::*;

    #[test]
    fn normal_is_the_initial_mode() {
        let bm = BlendModes::default();
        assert_eq!(bm.active(), Normal);
        assert!(bm.history().is_empty());
        assert_eq!(bm.flags(), (false, false, false));
    }

    #[test]
    fn activating_non_normal_modes_pushes_history_in_order() {
        let mut bm = BlendModes::default();
        assert!(bm.activate(Eraser));
        assert_eq!(bm.active(), Eraser);
        assert_eq!(bm.history(), &[Eraser]);
        assert!(bm.activate(LockAlpha));
        assert_eq!(bm.active(), LockAlpha);
        assert_eq!(bm.history(), &[Eraser, LockAlpha]);
        // 重复激活：源码 setter 不派发 changed，历史不变
        assert!(!bm.activate(LockAlpha));
        assert_eq!(bm.history(), &[Eraser, LockAlpha]);
        // 重新激活已经存在的模式：移除旧位置再压栈尾
        assert!(bm.activate(Eraser));
        assert_eq!(bm.history(), &[LockAlpha, Eraser]);
    }

    #[test]
    fn activating_normal_clears_history() {
        let mut bm = BlendModes::default();
        bm.activate(Eraser);
        bm.activate(Colorize);
        assert!(bm.activate(Normal));
        assert_eq!(bm.active(), Normal);
        assert!(bm.history().is_empty());
    }

    #[test]
    fn deactivating_current_mode_pops_to_previous_non_normal_mode() {
        let mut bm = BlendModes::default();
        bm.activate(Eraser);
        bm.activate(LockAlpha);
        // 取消锁 alpha → 回到橡皮（源码 _pop_history 跳过被移除的模式）
        assert!(bm.deactivate(LockAlpha));
        assert_eq!(bm.active(), Eraser);
        assert_eq!(bm.flags(), (true, false, false));
        // 再取消橡皮 → 历史空 → Normal
        assert!(bm.deactivate(Eraser));
        assert_eq!(bm.active(), Normal);
    }

    #[test]
    fn deactivating_a_non_current_mode_only_removes_it_from_history() {
        let mut bm = BlendModes::default();
        bm.activate(Eraser);
        bm.activate(LockAlpha);
        assert!(bm.deactivate(Eraser));
        assert_eq!(bm.active(), LockAlpha);
        assert_eq!(bm.history(), &[LockAlpha]);
        assert!(!bm.deactivate(Eraser));
        assert_eq!(bm.active(), LockAlpha);
    }

    #[test]
    fn brush_settings_pick_the_last_matching_mode() {
        let bm = BlendModes::activate_from_settings(true, true, true);
        assert_eq!(bm.active(), Colorize, "colorize 最后覆盖");
        let bm = BlendModes::activate_from_settings(true, true, false);
        assert_eq!(bm.active(), LockAlpha);
        let bm = BlendModes::activate_from_settings(true, false, false);
        assert_eq!(bm.active(), Eraser);
        let bm = BlendModes::activate_from_settings(false, false, false);
        assert_eq!(bm.active(), Normal);
    }

    #[test]
    fn action_ids_and_fill_indices_roundtrip() {
        for mode in BrushBlendMode::ALL {
            assert_eq!(BrushBlendMode::from_action_id(mode.action_id()), Some(mode));
            assert_eq!(BrushBlendMode::from_index(mode.index()), mode);
            assert_eq!(mode.index(), mode as u8);
        }
        assert_eq!(BrushBlendMode::from_action_id("Nope"), None);
        assert_eq!(BrushBlendMode::from_index(99), Normal);
    }

    #[test]
    fn setting_names_match_blendmodehandler() {
        assert_eq!(Normal.setting_name(), None);
        assert_eq!(Eraser.setting_name(), Some("eraser"));
        assert_eq!(LockAlpha.setting_name(), Some("lock_alpha"));
        assert_eq!(Colorize.setting_name(), Some("colorize"));
        assert_eq!(Normal.name(), "normal_mode");
    }

    fn mapped(base: f32) -> SettingValue {
        SettingValue {
            base_value: base,
            inputs: vec![pigment_engine::hokusai::InputMapping {
                input: pigment_engine::hokusai::BrushInput::Pressure,
                points: vec![(0.0, 0.0), (1.0, 0.5)],
            }],
            unknown_inputs: Default::default(),
        }
    }

    #[test]
    fn override_preserves_mapping_when_base_already_matches() {
        // 打开：base 0.95 已满足 > 0.9，保留原设置与曲线。
        let base = mapped(0.95);
        assert_eq!(override_setting_value(&base, &base, true), None);
        // 关闭：base 0.05 已满足 < 0.1，同样保留。
        let base = mapped(0.05);
        assert_eq!(override_setting_value(&base, &base, false), None);
    }

    #[test]
    fn forced_0_or_1_clears_input_mappings() {
        // 源码 MODE_FORCED_ON_SETTINGS / MODE_FORCED_OFF_SETTINGS 的第二项是 {}。
        let current = mapped(0.5);
        let unmodified = mapped(0.5);
        let on = override_setting_value(&current, &unmodified, true).unwrap();
        assert_eq!(on.base_value, 1.0);
        assert!(on.inputs.is_empty(), "强制打开必须清空输入映射");
        let off = override_setting_value(&current, &unmodified, false).unwrap();
        assert_eq!(off.base_value, 0.0);
        assert!(off.inputs.is_empty(), "强制关闭必须清空输入映射");
    }

    #[test]
    fn override_copies_unmodified_setting_including_its_curve() {
        let current = SettingValue::constant(0.5);
        let unmodified = mapped(0.95);
        let on = override_setting_value(&current, &unmodified, true).unwrap();
        assert_eq!(on, unmodified, "原始笔刷的 0.95 + 曲线应整体复制");
        let unmodified = mapped(0.05);
        let off = override_setting_value(&current, &unmodified, false).unwrap();
        assert_eq!(off, unmodified);
    }
}
