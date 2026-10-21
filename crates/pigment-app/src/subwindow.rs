//! 独立 OS 子窗口的元数据与偏好几何 —— `gui/windowing.py` 的 `SubWindow`。
//!
//! 源码的 `PreferencesWindow`、`BackgroundWindow`、`InputTestWindow`、
//! `BrushEditorWindow`、`BrushIconEditorWindow` 都是独立 Gtk.Window
//! （`application.py:422-430` 的 `get_subwindow()`）。本移植的 Plan A 外壳
//! （winit 0.31 + egui-wgpu）用第二个 winit 窗口 + 第二个 egui Context 承载
//! 这些窗口；本模块只放外壳与 app 共用的枚举、标题、默认尺寸和几何存取。
//!
//! 几何持久化是**超出源码的增强**：源码 `windowing.SubWindow.hide()`
//! 只记 `pre_hide_pos`（同一次运行内），不写偏好（`windowing.py:89-91`）。
//! 本移植把每个子窗口的 `(x,y,w,h)` 写进 `ui.subwindows.<key>`，下次启动恢复。

use eframe::egui;
use pigment_engine::prefs::Prefs;

use crate::app::PigmentApp;

/// 六个由原生窗口承载的非停靠子窗口。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    About,
    InputTest,
    Preferences,
    Background,
    BrushEditor,
    BrushIconEditor,
}

impl Kind {
    pub(crate) const ALL: [Kind; 6] = [
        Kind::About,
        Kind::InputTest,
        Kind::Preferences,
        Kind::Background,
        Kind::BrushEditor,
        Kind::BrushIconEditor,
    ];

    pub(crate) fn index(self) -> usize {
        match self {
            Kind::About => 0,
            Kind::InputTest => 1,
            Kind::Preferences => 2,
            Kind::Background => 3,
            Kind::BrushEditor => 4,
            Kind::BrushIconEditor => 5,
        }
    }

    /// 偏好键后缀（`ui.subwindows.<key>`）。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Kind::About => "about",
            Kind::InputTest => "input_test",
            Kind::Preferences => "preferences",
            Kind::Background => "background",
            Kind::BrushEditor => "brush_editor",
            Kind::BrushIconEditor => "brush_icon_editor",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Kind::About => "About MyPaint",
            Kind::InputTest => "Test Input Devices",
            Kind::Preferences => "Preferences",
            Kind::Background => "Background",
            Kind::BrushEditor => "Brush Settings Editor",
            // 源码 `BrushIconEditorWindow._TITLE_PREVIEWING`；编辑态在
            // [`Kind::title_for`] 里换成 `_TITLE_EDITING`。
            Kind::BrushIconEditor => "Brush Icon",
        }
    }

    /// 运行期标题（`brushiconeditor.py:35-53`：编辑态加 "(editing)"）。
    pub(crate) fn title_for(self, _app: &PigmentApp) -> String {
        match self {
            Kind::BrushIconEditor => "Brush Icon".to_string(),
            _ => self.title().to_string(),
        }
    }

    /// 默认尺寸：BrushEditor 照源码 `set_default_size(1000, 800)`
    /// （`brusheditor.py:134`）；图标编辑器由 256×256 画布 + 按钮撑开。
    pub(crate) fn default_size(self) -> (u32, u32) {
        match self {
            Kind::About => (560, 560),
            Kind::InputTest => (600, 460),
            Kind::Preferences => (930, 720),
            Kind::Background => (480, 580),
            Kind::BrushEditor => (1000, 800),
            Kind::BrushIconEditor => (640, 480),
        }
    }

    /// 动作开关当前是否打开（源码 `toggle_window_cb` 读 `get_visible()`）。
    ///
    /// 新 chrome 把 About / Input Test / Brush Editor 作为主窗口内的浮层
    /// 承载（`shell::dialogs`），因此这里统一返回 `false`：原生子窗口路径
    /// 保留给后续真正需要独立 OS 窗口的对话框，避免再次出现“菜单点了、
    /// 弹出的原生窗口却是空白”的状态。
    pub(crate) fn is_open(self, _app: &PigmentApp) -> bool {
        false
    }

    /// 关窗（X）时把 app 状态同步成关闭；等价 `hide_on_delete()`。
    pub(crate) fn set_closed(self, app: &mut PigmentApp) {
        match self {
            Kind::About => app.about_open = false,
            Kind::InputTest => app.input_test_open = false,
            Kind::Preferences => app.key_editor.capture = None,
            Kind::Background => {}
            Kind::BrushEditor => app.brush_editor_open = false,
            Kind::BrushIconEditor => {}
        }
    }
}

/// 读子窗口几何 `(x, y, w, h)`；没有/非法时 `None`。
pub(crate) fn geometry_from_prefs(prefs: &Prefs, kind: Kind) -> Option<(i32, i32, u32, u32)> {
    let obj = prefs
        .get_object("ui.subwindows")?
        .get(kind.key())?
        .as_object()?;
    let x = obj.get("x")?.as_i64()? as i32;
    let y = obj.get("y")?.as_i64()? as i32;
    let w = obj.get("w")?.as_u64()? as u32;
    let h = obj.get("h")?.as_u64()? as u32;
    if w == 0 || h == 0 {
        return None;
    }
    Some((x, y, w, h))
}

/// 写子窗口几何到 `ui.subwindows.<key>`，保留其它 key。
pub(crate) fn store_geometry(prefs: &mut Prefs, kind: Kind, x: i32, y: i32, w: u32, h: u32) {
    let mut root = prefs
        .get_object("ui.subwindows")
        .cloned()
        .unwrap_or_default();
    root.insert(
        kind.key().to_string(),
        serde_json::json!({ "x": x, "y": y, "w": w, "h": h }),
    );
    prefs.set("ui.subwindows", serde_json::Value::Object(root));
}

/// 子窗口内容尺寸（逻辑像素）。
pub(crate) fn screen_rect(size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(egui::Pos2::ZERO, size.max(egui::vec2(64.0, 64.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_prefs(tag: &str) -> Prefs {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "pigment-subwindow-prefs-{}-{tag}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        Prefs::load(p)
    }

    #[test]
    fn every_kind_has_a_distinct_key_and_default_size() {
        let mut keys = std::collections::HashSet::new();
        for k in Kind::ALL {
            assert!(keys.insert(k.key()), "重复 key: {:?}", k);
            let (w, h) = k.default_size();
            assert!(w >= 400 && h >= 400, "{k:?} 默认尺寸太小");
            assert!(!k.title().is_empty());
        }
    }

    #[test]
    fn geometry_roundtrips_and_preserves_other_windows() {
        let mut p = fresh_prefs("roundtrip");
        store_geometry(&mut p, Kind::Preferences, 10, 20, 700, 580);
        store_geometry(&mut p, Kind::InputTest, 30, 40, 600, 460);
        assert_eq!(
            geometry_from_prefs(&p, Kind::Preferences),
            Some((10, 20, 700, 580))
        );
        assert_eq!(
            geometry_from_prefs(&p, Kind::InputTest),
            Some((30, 40, 600, 460))
        );
        assert_eq!(geometry_from_prefs(&p, Kind::About), None);
    }

    #[test]
    fn malformed_geometry_is_ignored() {
        let mut p = fresh_prefs("malformed");
        p.set(
            "ui.subwindows",
            serde_json::json!({"about": {"x": 1, "y": 2, "w": 0, "h": 3}}),
        );
        assert_eq!(geometry_from_prefs(&p, Kind::About), None);
        p.set(
            "ui.subwindows",
            serde_json::json!({"about": {"x": "bad", "y": 2, "w": 3, "h": 4}}),
        );
        assert_eq!(geometry_from_prefs(&p, Kind::About), None);
    }
}
