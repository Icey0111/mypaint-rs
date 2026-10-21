//! `input.button_mapping` 的解析与本应用分派。
//!
//! 源码用 GTK 的 `Gtk.AccelMap` 绑定鼠标键（`gui/application.py` 的
//! `_apply_button_mapping_settings`），默认值见 `gui/userconfig.py:111-139`：
//!
//! ```text
//! Button2                 → PanViewMode
//! <Shift>Button2          → RotateViewMode
//! <Control>Button2        → ZoomViewMode
//! <Alt>Button2            → ZoomViewMode
//! <Control><Shift>Button2 → FrameEditMode
//! <Alt><Shift>Button2     → FrameEditMode
//! Button3                 → ShowPopupMenu
//! ```
//!
//! Windows 把 Button2/Button3 互换（`userconfig.py:127-139`），所以 Windows
//! 上默认是**中键弹主菜单、右键平移**。
//!
//! 本文件只做"键串 → 修饰键+按钮"的解析与精确匹配；具体动作在
//! `workspace::canvas` 里分派。

/// 一个鼠标绑定：修饰键 + 按钮号 → 动作 id。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonBinding {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// 1 = 左键、2 = 中键、3 = 右键（Gdk 的编号）。
    pub button: u32,
    pub action: String,
}

impl ButtonBinding {
    /// 键串是否精确匹配这组修饰键与按钮。
    pub fn matches(&self, button: u32, ctrl: bool, shift: bool, alt: bool) -> bool {
        self.button == button && self.ctrl == ctrl && self.shift == shift && self.alt == alt
    }

    /// 规范化后的键串 —— `gui/buttonmap.py:26-41` 的 `button_press_name()`。
    ///
    /// 用 GTK 的首选顺序 `<Control><Shift><Alt>ButtonN`（源码由
    /// `Gtk.accelerator_name()` 生成；`<Control>` 是旧版写法，新版会写成
    /// `<Primary>`，本项目的解析两种都收）。
    pub fn key_name(&self) -> String {
        format!(
            "{}{}{}Button{}",
            if self.ctrl { "<Control>" } else { "" },
            if self.shift { "<Shift>" } else { "" },
            if self.alt { "<Alt>" } else { "" },
            self.button
        )
    }

    /// 界面显示名 —— `gui/buttonmap.py:44-69` 的 `button_press_displayname()`。
    ///
    /// GTK 的 `accelerator_get_label()` 在中文/英文环境下都用
    /// `Ctrl` / `Shift` / `Alt` 拼加号；这里照英文标签（界面其余部分也是英文）。
    pub fn display_name(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push("Button");
        let prefix = if parts.len() > 1 {
            format!("{}+", parts[..parts.len() - 1].join("+"))
        } else {
            String::new()
        };
        format!("{prefix}Button{}", self.button)
    }
}

/// 由鼠标键 + 修饰键构造一条规范化绑定（编辑器捕获时用）。
///
/// 按钮号为 GDK 编号：1 = 左键、2 = 中键、3 = 右键。
pub fn binding_for(button: u32, ctrl: bool, shift: bool, alt: bool) -> ButtonBinding {
    ButtonBinding {
        ctrl,
        shift,
        alt,
        button,
        action: String::new(),
    }
}

/// 解析单条键串，例如 `<Control><Shift>Button2` 或 `Button3`。
pub fn parse_binding(key: &str, action: &str) -> Option<ButtonBinding> {
    let mut rest = key.trim().to_string();
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    loop {
        if let Some(r) =
            strip_prefix_ci(&rest, "<control>").or_else(|| strip_prefix_ci(&rest, "<ctrl>"))
        {
            ctrl = true;
            rest = r;
        } else if let Some(r) = strip_prefix_ci(&rest, "<shift>") {
            shift = true;
            rest = r;
        } else if let Some(r) = strip_prefix_ci(&rest, "<alt>") {
            alt = true;
            rest = r;
        } else if let Some(r) =
            strip_prefix_ci(&rest, "ctrl+").or_else(|| strip_prefix_ci(&rest, "control+"))
        {
            ctrl = true;
            rest = r;
        } else if let Some(r) = strip_prefix_ci(&rest, "shift+") {
            shift = true;
            rest = r;
        } else if let Some(r) = strip_prefix_ci(&rest, "alt+") {
            alt = true;
            rest = r;
        } else {
            break;
        }
    }
    let button = match rest.trim().to_ascii_lowercase().as_str() {
        "button1" => 1,
        "button2" => 2,
        "button3" => 3,
        "button4" => 4,
        _ => return None,
    };
    Some(ButtonBinding {
        ctrl,
        shift,
        alt,
        button,
        action: action.to_string(),
    })
}

fn strip_prefix_ci(s: &str, prefix: &str) -> Option<String> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(s[prefix.len()..].to_string())
    } else {
        None
    }
}

/// 解析 `input.button_mapping` 的 `{键串: 动作}` 表。
pub fn parse_mapping<'a>(
    pairs: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<ButtonBinding> {
    pairs
        .into_iter()
        .filter_map(|(k, v)| parse_binding(k, v))
        .collect()
}

/// 精确匹配第一个绑定（源码 AccelMap 也是精确修饰键匹配）。
pub fn resolve<'a>(
    bindings: &'a [ButtonBinding],
    button: u32,
    ctrl: bool,
    shift: bool,
    alt: bool,
) -> Option<&'a str> {
    bindings
        .iter()
        .find(|b| b.matches(button, ctrl, shift, alt))
        .map(|b| b.action.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gtk_and_plus_syntax() {
        let b = parse_binding("<Control><Shift>Button2", "FrameEditMode").unwrap();
        assert!(b.ctrl && b.shift && !b.alt);
        assert_eq!(b.button, 2);
        let b = parse_binding("Ctrl+Alt+Button3", "X").unwrap();
        assert!(b.ctrl && b.alt);
        assert_eq!(b.button, 3);
        assert!(parse_binding("<Control>ScrollUp", "X").is_none());
    }

    #[test]
    fn windows_default_mapping_swaps_button2_and_button3() {
        // 与 `prefs::windows_button_mapping` 一致
        let pairs = [
            ("Button2", "ShowPopupMenu"),
            ("<Shift>Button3", "RotateViewMode"),
            ("<Control>Button3", "ZoomViewMode"),
            ("<Alt>Button3", "ZoomViewMode"),
            ("<Control><Shift>Button3", "FrameEditMode"),
            ("<Alt><Shift>Button3", "FrameEditMode"),
            ("Button3", "PanViewMode"),
        ];
        let b = parse_mapping(pairs);
        assert_eq!(resolve(&b, 2, false, false, false), Some("ShowPopupMenu"));
        assert_eq!(resolve(&b, 3, false, false, false), Some("PanViewMode"));
        assert_eq!(resolve(&b, 3, true, false, false), Some("ZoomViewMode"));
        assert_eq!(resolve(&b, 3, false, true, false), Some("RotateViewMode"));
        assert_eq!(resolve(&b, 3, true, true, false), Some("FrameEditMode"));
        // 未绑定的组合不匹配
        assert_eq!(resolve(&b, 3, true, true, true), None);
    }

    #[test]
    fn custom_mappings_can_reach_brush_resize_and_frame_edit() {
        let pairs = [
            ("<Control>Button1", "BrushResizeMode"),
            ("<Control><Shift>Button3", "FrameEditMode"),
        ];
        let b = parse_mapping(pairs);
        assert_eq!(resolve(&b, 1, true, false, false), Some("BrushResizeMode"));
        assert_eq!(resolve(&b, 3, true, true, false), Some("FrameEditMode"));
    }

    #[test]
    fn key_and_display_names_follow_gtk_button_map() {
        let b = binding_for(2, true, true, false);
        assert_eq!(b.key_name(), "<Control><Shift>Button2");
        assert_eq!(b.display_name(), "Ctrl+Shift+Button2");
        assert!(b.matches(2, true, true, false));
        assert!(!b.matches(2, true, true, true));
        let b = binding_for(1, false, false, true);
        assert_eq!(b.key_name(), "<Alt>Button1");
        assert_eq!(b.display_name(), "Alt+Button1");
        let b = binding_for(3, false, false, false);
        assert_eq!(b.key_name(), "Button3");
        assert_eq!(b.display_name(), "Button3");
    }

    #[test]
    fn linux_default_mapping_uses_middle_for_view_tools() {
        let pairs = [
            ("Button2", "PanViewMode"),
            ("<Shift>Button2", "RotateViewMode"),
            ("<Control>Button2", "ZoomViewMode"),
            ("Button3", "ShowPopupMenu"),
        ];
        let b = parse_mapping(pairs);
        assert_eq!(resolve(&b, 2, false, false, false), Some("PanViewMode"));
        assert_eq!(resolve(&b, 2, false, true, false), Some("RotateViewMode"));
        assert_eq!(resolve(&b, 3, false, false, false), Some("ShowPopupMenu"));
    }
}
