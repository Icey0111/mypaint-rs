//! 笔刷图标编辑器 —— `gui/brushiconeditor.py` 的 egui 版。
//!
//! 源码是「256×256 的 TDW + FreehandMode + 一个独立 Document」：
//! 预览模式只显示当前笔刷的 `preview`；Edit 后把图标锁住，用户可用**任何**
//! 笔刷/颜色在上面画；Clear/Revert/Auto/Save 四个动作，Save 写回
//! `_prev.png`（`ManagedBrush.save()` 同时写 `.myb`）。
//!
//! 本实现用一块 128×128 的 `PigmentSurface` 承担画布（不建第二份 Document/
//! 模式栈），指针事件直接喂 `Brush::stroke_to`；显示放大 2 倍（`_SCALE = 2`）。
//!
//! 记账：没有图标编辑的撤销/重做（源码有独立 document command stack）。

use eframe::egui;
use pigment_engine::hokusai::{Brush, BrushState, TiledSurface};
use pigment_surface::PigmentSurface;

/// 图标尺寸（`brushmanager.PREVIEW_W/H = 128`）。
pub const ICON_SIZE: usize = 128;
/// 显示放大倍数（`BrushIconEditor._SCALE = 2`）。
const SCALE: f32 = 2.0;

#[derive(Default)]
pub struct IconEditorState {
    pub open: bool,
    pub editing: bool,
    /// 正在编辑的笔刷 key。
    pub key: Option<String>,
    pub surface: PigmentSurface,
    pub state: BrushState,
    pub tex: Option<egui::TextureHandle>,
    pub dirty: bool,
    pub status: String,
}

impl IconEditorState {
    /// 打开并载入某支笔刷的预览（预览优先取随附 PNG，缺了再渲染）。
    pub fn open(&mut self, key: String, preview: Vec<u8>) {
        self.open = true;
        self.editing = false;
        self.key = Some(key);
        self.dirty = false;
        self.status.clear();
        self.load_rgba(&preview);
    }

    pub fn load_rgba(&mut self, rgba: &[u8]) {
        let mut surface = PigmentSurface::new();
        surface.load_from_rgba8(rgba, 0, 0, ICON_SIZE as u32, ICON_SIZE as u32);
        self.surface = surface;
        self.state = BrushState::default();
        self.state.reset();
        self.tex = None;
    }

    /// 开始新的一笔（模式栈在 ButtonPress 时也做同样的事）。
    pub fn begin_stroke(&mut self) {
        self.state = BrushState::default();
        self.state.reset();
    }

    /// 从画布取 128×128 不透明 RGBA（`render_layer_as_pixbuf(..., alpha=False)`）。
    pub fn image_rgba(&self) -> Vec<u8> {
        self.surface
            .render_rect_rgba8((0, 0, ICON_SIZE as i32, ICON_SIZE as i32), true, [255, 255, 255])
    }

    pub fn clear(&mut self) {
        let white = vec![255u8; ICON_SIZE * ICON_SIZE * 4];
        self.load_rgba(&white);
        self.dirty = true;
    }

    /// 用当前笔刷设置重画默认图标（`render_brush_preview_pixbuf`）。
    pub fn auto(&mut self, brush: &Brush) {
        let rgba = crate::thumb::render(brush);
        self.load_rgba(&rgba);
        self.dirty = true;
    }

    /// 一笔的单个事件；坐标是 0..128 的图标坐标。
    pub fn paint(&mut self, brush: &Brush, pos: [f32; 2], dt: f32) {
        if !self.editing {
            return;
        }
        let b = brush.clone();
        let mut state = self.state.clone();
        self.surface.begin_atomic();
        b.stroke_to(
            &mut state,
            &mut self.surface,
            pos[0],
            pos[1],
            0.5,
            0.0,
            0.0,
            dt.max(1.0) as f64,
        );
        self.surface.end_atomic();
        self.state = state;
        self.dirty = true;
    }

    /// 把 surface 重新上传成纹理（每次改动后调）。
    pub fn refresh_texture(&mut self, ctx: &egui::Context) {
        let rgba = self.image_rgba();
        let img = egui::ColorImage::from_rgba_unmultiplied([ICON_SIZE, ICON_SIZE], &rgba);
        self.tex = Some(ctx.load_texture(
            "brush-icon-editor",
            img,
            egui::TextureOptions::NEAREST,
        ));
    }
}

/// 窗口里的一个按钮动作（闭包里只记动作，借不到 `PigmentApp`）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    Edit,
    Revert,
    Clear,
    Auto,
    Save,
}

impl crate::app::PigmentApp {
    /// 打开图标编辑器（BrushEditor 的 "Edit Icon…"）。
    pub(crate) fn open_icon_editor(&mut self) {
        let Some(entry) = self.brushes.selected_entry().cloned() else {
            return;
        };
        let preview = if let Some(bytes) = entry.preview_png() {
            image::load_from_memory(&bytes)
                .ok()
                .map(|img| img.to_rgba8())
                .map(|img| {
                    let img = image::imageops::resize(
                        &img,
                        ICON_SIZE as u32,
                        ICON_SIZE as u32,
                        image::imageops::FilterType::Lanczos3,
                    );
                    img.into_raw()
                })
                .unwrap_or_else(|| crate::thumb::render(&self.base_brush))
        } else {
            crate::thumb::render(&self.base_brush)
        };
        self.icon_editor.open(entry.key.clone(), preview);
    }

    /// 保存图标：只改 `_prev.png`（`ManagedBrush.save` 同路径会写 `.myb`）。
    pub(crate) fn save_icon_editor(&mut self) {
        let Some(key) = self.icon_editor.key.clone() else {
            return;
        };
        let rgba = self.icon_editor.image_rgba();
        match self.brushes.library.save_brush_preview(
            &key,
            (ICON_SIZE as u32, ICON_SIZE as u32, &rgba),
        ) {
            Ok(idx) => {
                self.brushes.refresh_after_save(idx);
                self.icon_editor.editing = false;
                self.icon_editor.dirty = false;
                self.icon_editor.status = "Saved".to_string();
            }
            Err(e) => {
                self.icon_editor.status = format!("保存失败: {e}");
                eprintln!("保存笔刷图标失败: {e}");
            }
        }
    }

    /// 渲染图标编辑器窗口（在 brush editor 内容之后调）。
    pub(crate) fn icon_editor_window(&mut self, ctx: &egui::Context) {
        if !self.icon_editor.open {
            return;
        }
        let brush = self.brush.clone();
        let mut open = true;
        let mut action = Action::None;
        let title = if self.icon_editor.editing {
            "Brush Icon (editing)"
        } else {
            "Brush Icon"
        };
        egui::Window::new(title)
            .id(egui::Id::unique("brush_icon_editor"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let ed = &mut self.icon_editor;
                ui.horizontal_top(|ui| {
                    let side = ICON_SIZE as f32 * SCALE;
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(side, side),
                        egui::Sense::click_and_drag(),
                    );
                    if ed.tex.is_none() {
                        ed.refresh_texture(ui.ctx());
                    }
                    if let Some(tex) = &ed.tex {
                        ui.painter().image(
                            tex.id(),
                            rect,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );
                    } else {
                        ui.painter().rect_filled(rect, 0.0, egui::Color32::WHITE);
                    }
                    ui.painter().rect_stroke(
                        rect,
                        0.0,
                        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                        egui::StrokeKind::Middle,
                    );

                    if ed.editing {
                        let local = |p: egui::Pos2| {
                            [
                                (p.x - rect.min.x) / SCALE,
                                (p.y - rect.min.y) / SCALE,
                            ]
                        };
                        if resp.drag_started() {
                            if let Some(p) = resp.interact_pointer_pos() {
                                ed.begin_stroke();
                                ed.paint(&brush, local(p), 1.0);
                                ed.refresh_texture(ui.ctx());
                            }
                        } else if resp.dragged() {
                            if let Some(p) = resp.interact_pointer_pos() {
                                ed.paint(&brush, local(p), 16.0);
                                ed.refresh_texture(ui.ctx());
                            }
                        } else if resp.clicked() {
                            if let Some(p) = resp.interact_pointer_pos() {
                                ed.begin_stroke();
                                ed.paint(&brush, local(p), 1.0);
                                ed.refresh_texture(ui.ctx());
                            }
                        }
                    }

                    ui.vertical(|ui| {
                        ui.add_enabled_ui(!ed.editing, |ui| {
                            if ui.button("Edit").clicked() {
                                action = Action::Edit;
                            }
                        });
                        ui.add_enabled_ui(ed.editing, |ui| {
                            if ui.button("Revert").clicked() {
                                action = Action::Revert;
                            }
                            if ui.button("Clear").clicked() {
                                action = Action::Clear;
                            }
                            if ui.button("Auto").clicked() {
                                action = Action::Auto;
                            }
                            if ui.button("Save").clicked() {
                                action = Action::Save;
                            }
                        });
                    });
                });
                let key = ed.key.clone().unwrap_or_default();
                let state = if ed.editing {
                    if ed.dirty {
                        "editing — changes not yet saved"
                    } else {
                        "editing — paint with any brush or color"
                    }
                } else {
                    "Click Edit to make changes to the icon"
                };
                ui.label(format!("{key}\n{state}"));
                if !ed.status.is_empty() {
                    ui.label(egui::RichText::new(&ed.status).weak().small());
                }
            });
        self.icon_editor.open = open;
        match action {
            Action::Edit => self.icon_editor.editing = true,
            Action::Revert => {
                self.icon_editor.editing = false;
                self.icon_editor.dirty = false;
                self.open_icon_editor();
            }
            Action::Clear => self.icon_editor.clear(),
            Action::Auto => {
                let brush = self.base_brush.clone();
                self.icon_editor.auto(&brush);
            }
            Action::Save => self.save_icon_editor(),
            Action::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::brushlib::default_brush;

    #[test]
    fn clear_auto_and_image_are_128_opaque() {
        let mut ed = IconEditorState::default();
        ed.clear();
        let img = ed.image_rgba();
        assert_eq!(img.len(), ICON_SIZE * ICON_SIZE * 4);
        assert!(img.chunks_exact(4).all(|p| p[3] == 255), "图标必须不透明");
        assert!(img.chunks_exact(4).all(|p| p[0] == 255), "Clear 后应为白底");

        ed.auto(&default_brush());
        let img = ed.image_rgba();
        assert_eq!(img.len(), ICON_SIZE * ICON_SIZE * 4);
        assert!(
            img.chunks_exact(4).any(|p| p[0] != 255 || p[1] != 255 || p[2] != 255),
            "Auto 应画出默认预览"
        );
    }

    #[test]
    fn painting_modifies_the_icon() {
        let mut ed = IconEditorState::default();
        ed.clear();
        let before = ed.image_rgba();
        ed.editing = true;
        let lib = pigment_engine::brushlib::BrushLibrary::embedded();
        let mut brush = lib.entries[lib.find("classic/pencil").unwrap()].load().unwrap();
        brush.get_mut(pigment_engine::hokusai::BrushSetting::ColorH).base_value = 0.0;
        brush.get_mut(pigment_engine::hokusai::BrushSetting::ColorS).base_value = 0.0;
        brush.get_mut(pigment_engine::hokusai::BrushSetting::ColorV).base_value = 0.0;
        ed.paint(&brush, [20.0, 20.0], 0.016);
        ed.paint(&brush, [60.0, 60.0], 0.016);
        ed.paint(&brush, [100.0, 100.0], 0.016);
        let after = ed.image_rgba();
        assert_ne!(before, after, "画一笔后图标像素应变化");
    }
}

