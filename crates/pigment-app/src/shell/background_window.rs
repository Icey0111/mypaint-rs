//! 背景选择窗口 —— `gui/backgroundwindow.py` 的 egui 移植。
//!
//! 源码是 GTK 的 `Gtk.Dialog` + 两页 `Gtk.Notebook`：
//!
//! | 源码 | 本文件 |
//! |---|---|
//! | `BackgroundWindow`（`:45-151`） | [`PigmentApp::background_window`] + [`BackgroundWindowState`] |
//! | `BackgroundList`（`:154-318`） | [`list_backgrounds`] + 缩略图缓存 |
//! | `load_background`（`:359-481`） | 引擎 `document::background_from_file`（缩放/压平已在引擎） |
//! | "Save as Default"（`:120-129`） | [`save_current_as_default`] |
//! | "Add color to Patterns"（`:137-151`） | [`next_color_path`] + 写 64×64 PNG |
//!
//! # 与源码的差异
//!
//! - 源码用 `GdkPixbuf` 预览、`pixbuflist` 网格；这里用 egui 的 48×48 纹理 +
//!   纵向列表（没有网格控件，交互等价：点一项即换背景）。
//! - 内置 fallback 纸纹不在列表里（源码里 stock 目录还有别的背景图，本项目
//!   只打包了 `mamurk_e_1.png`）；列表顶部给一个 "Default Paper" 入口。
//! - `default.png` 照源码从列表里剔除（`:179-183`），只能通过 "Save as Default" 写。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use eframe::egui;
use pigment_engine::document::background_from_file;
use pigment_surface::layer::BackgroundImage;

/// 背景图后缀（`backgroundwindow.py:156`）。
const SUFFIXES: [&str; 3] = [".jpg", ".jpeg", ".png"];
/// 默认背景文件名（`backgroundwindow.py:35`）。
pub(crate) const DEFAULT_BACKGROUND: &str = "default.png";
/// 背景图子目录（`backgroundwindow.py:37`）。
pub(crate) const BACKGROUNDS_SUBDIR: &str = "backgrounds";
/// 缩略图边长（源码 `PixbufList(N, N)` 是 64；这里为了侧栏宽度用 48）。
const THUMB: usize = 48;

/// 窗口状态（egui 立即模式：控件没有对象，页签/选择状态由应用持有）。
#[derive(Default)]
pub(crate) struct BackgroundWindowState {
    pub(crate) open: bool,
    /// 0 = Pattern，1 = Color（`Gtk.Notebook` 的两页）。
    pub(crate) tab: usize,
    /// 当前背景是否被本窗口改过（决定 "Save as Default" 是否可用）。
    pub(crate) dirty: bool,
    /// Color 页当前颜色（sRGB8）。
    pub(crate) color: [u8; 3],
    /// 已扫描到的背景文件（不含 `default.png`）。
    pub(crate) files: Vec<PathBuf>,
    /// 载入/保存错误，显示在窗口底部。
    pub(crate) error: Option<String>,
    /// 48×48 缩略图缓存（按路径）。
    thumbnails: HashMap<PathBuf, egui::TextureHandle>,
}

impl BackgroundWindowState {
    /// 打开/关闭窗口；打开时重新扫描目录（源码 `_realize_cb` 的 `initialize`）。
    pub(crate) fn toggle(&mut self, dir: &Path) {
        self.open = !self.open;
        if self.open {
            self.files = list_backgrounds(dir);
            self.dirty = false;
            self.error = None;
        }
    }

    /// 重新扫描（"Add color to Patterns" 之后）。
    pub(crate) fn refresh(&mut self, dir: &Path) {
        self.files = list_backgrounds(dir);
    }
}

/// 递归列出背景图（`BackgroundList._list_dir`，`:191-208`）。
///
/// 排序照源码：先按 mtime 升序；`default.png` 从列表剔除（`:179-183`）。
pub(crate) fn list_backgrounds(base: &Path) -> Vec<PathBuf> {
    let dir = base.join(BACKGROUNDS_SUBDIR);
    let mut out = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !SUFFIXES.iter().any(|s| name.ends_with(s)) {
                continue;
            }
            if name == DEFAULT_BACKGROUND {
                continue;
            }
            out.push(path);
        }
    }
    out.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
    });
    out
}

/// 下一个可用的 `colorNN.png` 路径（`backgroundwindow.py:137-147`）。
pub(crate) fn next_color_path(base: &Path) -> PathBuf {
    let dir = base.join(BACKGROUNDS_SUBDIR);
    for i in 1..1000 {
        let p = dir.join(format!("color{i:02}.png"));
        if !p.exists() {
            return p;
        }
    }
    dir.join("color999.png")
}

/// 把一张 64×64 纯色写成 PNG（"Add color to Patterns"）。
pub(crate) fn write_color_png(path: &Path, rgb: [u8; 3]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("创建背景目录 {} 失败：{e}", dir.display()))?;
    }
    let n = 64usize * 64;
    let mut px = Vec::with_capacity(n * 4);
    for _ in 0..n {
        px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
    }
    pigment_surface::png::write_rgba8_png(path, px, 64, 64)
        .map_err(|e| format!("写背景 {} 失败：{e}", path.display()))
}

/// 把当前背景存成 `<user>/backgrounds/default.png`（"Save as Default"）。
///
/// 当前背景是图片时直接写它的原始 RGBA8；是纯色档时写 64×64 纯色图。
pub(crate) fn save_current_as_default(
    base: &Path,
    image: Option<&BackgroundImage>,
    color: [u8; 3],
) -> Result<PathBuf, String> {
    let path = base.join(BACKGROUNDS_SUBDIR).join(DEFAULT_BACKGROUND);
    if let Some(img) = image {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("创建背景目录 {} 失败：{e}", dir.display()))?;
        }
        pigment_surface::png::write_rgba8_png(
            &path,
            img.rgba8().to_vec(),
            img.width(),
            img.height(),
        )
        .map_err(|e| format!("写背景 {} 失败：{e}", path.display()))?;
    } else {
        write_color_png(&path, color)?;
    }
    Ok(path)
}

/// 取缩略图纹理（首次调用时解码）。
fn thumbnail(
    ctx: &egui::Context,
    path: &Path,
    cache: &mut HashMap<PathBuf, egui::TextureHandle>,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.get(path) {
        return Some(t.clone());
    }
    let img = background_from_file(path).ok()?;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let src = img.rgba8();
    let mut px = vec![0u8; THUMB * THUMB * 4];
    for y in 0..THUMB {
        for x in 0..THUMB {
            let sx = x * w / THUMB;
            let sy = y * h / THUMB;
            let si = (sy * w + sx) * 4;
            let di = (y * THUMB + x) * 4;
            px[di..di + 4].copy_from_slice(&src[si..si + 4]);
            px[di + 3] = 255;
        }
    }
    let color = egui::ColorImage::from_rgba_unmultiplied([THUMB, THUMB], &px);
    let tex = ctx.load_texture(
        format!("background-thumb-{}", path.display()),
        color,
        egui::TextureOptions::LINEAR,
    );
    cache.insert(path.to_path_buf(), tex.clone());
    Some(tex)
}

impl crate::app::PigmentApp {
    /// 背景选择窗口（`BackgroundWindow` 动作；`resources.xml:1345-1352`）。
    pub(crate) fn background_window(&mut self, ctx: &egui::Context) {
        if !self.background_window.open {
            return;
        }
        let base = pigment_engine::prefs::config_dir();
        let mut open = true;
        let mut close = false;
        let mut choose: Option<PathBuf> = None;
        let mut reset_default = false;
        let mut add_color = false;
        let mut save_default = false;
        let mut new_color: Option<[u8; 3]> = None;
        let files = self.background_window.files.clone();
        let mut thumbnails = self.background_window.thumbnails.clone();
        let tab = self.background_window.tab;
        let mut tab_now = tab;
        let color = self.background_window.color;
        let dirty = self.background_window.dirty;

        egui::Window::new("Background")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(360.0)
            .default_height(480.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.selectable_label(tab_now == 0, "Pattern").clicked() {
                        tab_now = 0;
                    }
                    if ui.selectable_label(tab_now == 1, "Color").clicked() {
                        tab_now = 1;
                    }
                });
                ui.separator();
                match tab_now {
                    0 => {
                        if ui
                            .button("Default Paper")
                            .on_hover_text("恢复内置纸纹（或用户 default.png）")
                            .clicked()
                        {
                            reset_default = true;
                        }
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .show(ui, |ui| {
                                if files.is_empty() {
                                    ui.label(
                                        egui::RichText::new(
                                            "没有找到背景图；把 PNG/JPEG 放进 backgrounds/ 目录",
                                        )
                                        .italics()
                                        .weak(),
                                    );
                                    ui.label(
                                        egui::RichText::new(base.join(BACKGROUNDS_SUBDIR).display().to_string())
                                            .small()
                                            .weak(),
                                    );
                                }
                                for path in &files {
                                    let name = path
                                        .file_name()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("?")
                                        .to_string();
                                    ui.horizontal(|ui| {
                                        if let Some(tex) = thumbnail(ui.ctx(), path, &mut thumbnails) {
                                            ui.add(
                                                egui::Image::new(&tex)
                                                    .max_size(egui::vec2(32.0, 32.0)),
                                            );
                                        }
                                        if ui.selectable_label(false, name).clicked() {
                                            choose = Some(path.clone());
                                        }
                                    });
                                }
                            });
                    }
                    _ => {
                        let mut c = color;
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            new_color = Some(c);
                        }
                        ui.label(format!(
                            "sRGB: #{:02X}{:02X}{:02X}",
                            c[0], c[1], c[2]
                        ));
                        ui.separator();
                        if ui
                            .button("Add color to Patterns")
                            .on_hover_text("把当前颜色存成 64×64 的 colorNN.png")
                            .clicked()
                        {
                            add_color = true;
                        }
                    }
                }
                if let Some(e) = &self.background_window.error {
                    ui.separator();
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(dirty, egui::Button::new("Save as Default"))
                        .on_hover_text("写成 backgrounds/default.png，下次启动沿用")
                        .clicked()
                    {
                        save_default = true;
                    }
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                });
            });

        self.background_window.tab = tab_now;
        self.background_window.thumbnails = thumbnails;
        if reset_default {
            self.doc.reset_background_from(Some(&base));
            self.background_window.dirty = true;
        }
        if let Some(path) = choose {
            match self.doc.set_background_from_file(&path) {
                Ok(()) => {
                    self.background_window.dirty = true;
                    self.background_window.error = None;
                }
                Err(e) => self.background_window.error = Some(e),
            }
        }
        if let Some(c) = new_color {
            self.doc.layers_mut().set_background_color(c);
            self.background_window.color = c;
            self.background_window.dirty = true;
        }
        if add_color {
            let path = next_color_path(&base);
            match write_color_png(&path, self.background_window.color) {
                Ok(()) => {
                    self.background_window.refresh(&base);
                    self.background_window.dirty = true;
                    self.background_window.error = None;
                }
                Err(e) => self.background_window.error = Some(e),
            }
        }
        if save_default {
            let image = self.doc.layers().background_image().cloned();
            match save_current_as_default(
                &base,
                image.as_deref(),
                self.background_window.color,
            ) {
                Ok(_) => {
                    self.background_window.dirty = false;
                    self.background_window.error = None;
                }
                Err(e) => self.background_window.error = Some(e),
            }
        }
        if close || !open {
            self.background_window.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pigment-bgwin-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join(BACKGROUNDS_SUBDIR)).unwrap();
        p
    }

    #[test]
    fn list_backgrounds_recurses_and_skips_default() {
        let dir = tmpdir("list");
        let bgs = dir.join(BACKGROUNDS_SUBDIR);
        std::fs::create_dir_all(bgs.join("sub")).unwrap();
        for p in [
            bgs.join("a.png"),
            bgs.join("sub").join("b.jpg"),
            bgs.join(DEFAULT_BACKGROUND),
            bgs.join("notes.txt"),
        ] {
            // 内容不合法也没关系：列表只按后缀收，不在这里解码
            std::fs::write(&p, b"not-an-image").unwrap();
        }
        let files = list_backgrounds(&dir);
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"a.png".to_string()));
        assert!(names.contains(&"b.jpg".to_string()));
        assert!(!names.contains(&DEFAULT_BACKGROUND.to_string()));
        assert!(!names.contains(&"notes.txt".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_color_path_increments_and_skips_existing() {
        let dir = tmpdir("color");
        assert!(next_color_path(&dir).ends_with("color01.png"));
        std::fs::write(dir.join(BACKGROUNDS_SUBDIR).join("color01.png"), b"x").unwrap();
        assert!(next_color_path(&dir).ends_with("color02.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_default_round_trips_through_the_engine_loader() {
        let dir = tmpdir("save");
        let path = save_current_as_default(&dir, None, [12, 34, 56]).unwrap();
        assert!(path.ends_with(DEFAULT_BACKGROUND));
        let img = background_from_file(&path).unwrap();
        assert_eq!(img.average_rgb(), [12, 34, 56]);

        let mut doc = pigment_engine::Document::new();
        doc.reset_background_from(Some(&dir));
        assert_eq!(
            doc.layers().background_image().unwrap().average_rgb(),
            [12, 34, 56]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
