//! 笔刷库面板 —— MyPaint 式的扁平笔刷列表。
//!
//! - 官方包 196 支（CC0，编译期内嵌）+ 用户笔刷目录扫描结果
//! - 分组过滤条（顺序与展示名取自官方 `order.conf`）+ 名称搜索
//! - `ScrollArea::show_rows` 虚拟滚动：**只为可见行渲染缩略图**，
//!   196 支也不可能一次性付渲染代价
//! - 缩略图缓存按「颜色」失效：换色后重绘，但永远只重绘可见的那屏

use std::collections::HashMap;

use eframe::egui;
use pigment_engine::brushlib::{BrushEntry, BrushLibrary};
use pigment_engine::hokusai::Brush;

use crate::thumb;

/// 列表行高（含间距）。
const ROW_H: f32 = 21.0;
/// 缩略图显示宽度（像素）。
const THUMB_W: f32 = 40.0;

/// MyPaint 的启动默认笔刷 —— 对齐 `gui/brushmanager.py` 的 `_DEFAULT_BRUSH`。
pub const DEFAULT_BRUSH: &str = "Dieterle/Fan#1";
/// MyPaint 的默认橡皮 —— 对齐 `_DEFAULT_ERASER`。
///
/// 官方包实际目录名是全小写 `deevad/`，order.conf 里的分组展示名才是 `Deevad`。
#[allow(dead_code)]
pub const DEFAULT_ERASER: &str = "deevad/kneaded_eraser_large";
/// `DEFAULT_BRUSH` 缺失时的兜底（官方包被裁剪等情况）。
const FALLBACK_BRUSH: &str = "classic/dry_brush";

pub struct BrushPanel {
    pub library: BrushLibrary,
    /// 当前选中项在 `library.entries` 中的下标。
    pub selected: usize,
    /// 名称搜索关键字。
    pub search: String,
    /// 分组过滤（`None` = 全部）。
    pub group: Option<String>,
    /// 缩略图缓存：library 下标 → 纹理。
    thumbnails: HashMap<usize, egui::TextureHandle>,
    /// 缩略图当前渲染所用的颜色（变化即整屏失效重绘）。
    thumb_color: Option<[f32; 3]>,
    /// 过滤后的 library 下标列表。
    visible: Vec<usize>,
}

impl BrushPanel {
    pub fn new() -> Self {
        let library = BrushLibrary::load();
        // 对齐 MyPaint：默认选中 Dieterle/Fan#1（`brushmanager.py:_DEFAULT_BRUSH`）
        let selected = library
            .find(DEFAULT_BRUSH)
            .or_else(|| library.find(FALLBACK_BRUSH))
            .unwrap_or(0);
        let mut p = Self {
            library,
            selected,
            search: String::new(),
            group: None,
            thumbnails: HashMap::new(),
            thumb_color: None,
            visible: Vec::new(),
        };
        p.rebuild_visible();
        p
    }

    pub fn selected_entry(&self) -> Option<&BrushEntry> {
        self.library.entries.get(self.selected)
    }

    /// 解析当前选中笔刷（列表里解析失败极少见，此处以 `Brush::new()` 兜底）。
    pub fn load_selected(&self) -> Result<Brush, String> {
        self.library
            .entries
            .get(self.selected)
            .ok_or_else(|| "笔刷库为空".to_string())?
            .load()
            .map_err(|e| e.to_string())
    }

    /// 在过滤结果中前后移动选择，返回是否发生变化。
    pub fn step(&mut self, delta: isize) -> bool {
        if self.visible.is_empty() {
            return false;
        }
        let cur = self.visible.iter().position(|&i| i == self.selected);
        let next = match cur {
            Some(p) => (p as isize + delta).rem_euclid(self.visible.len() as isize) as usize,
            None => 0,
        };
        let target = self.visible[next];
        if target == self.selected {
            return false;
        }
        self.selected = target;
        true
    }

    fn rebuild_visible(&mut self) {
        let needle = self.search.trim().to_lowercase();
        self.visible = self
            .library
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                self.group.as_deref().is_none_or(|g| e.group == g)
                    && (needle.is_empty()
                        || e.display.to_lowercase().contains(&needle)
                        || e.name.to_lowercase().contains(&needle)
                        || e.key.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();
        // 当前选中项被过滤掉时，自动切到第一条可见项（避免"选中了但看不见"）
        if !self.visible.is_empty() && !self.visible.contains(&self.selected) {
            self.selected = self.visible[0];
        }
    }

    /// 绘制面板。返回 `true` 表示**选中笔刷发生变化**（调用方需重载笔刷）。
    pub fn show(&mut self, ui: &mut egui::Ui, color: [f32; 3]) -> bool {
        let mut changed = false;

        // 颜色变化 → 缩略图整屏失效（下次渲染时按需重绘）
        if self.thumb_color != Some(color) {
            self.thumbnails.clear();
            self.thumb_color = Some(color);
        }

        ui.horizontal(|ui| {
            ui.heading("笔刷");
            ui.label(
                egui::RichText::new(format!("{} 支", self.library.len()))
                    .weak()
                    .small(),
            );
            if !self.library.errors.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{} 个失败", self.library.errors.len()))
                        .color(egui::Color32::from_rgb(220, 120, 80))
                        .small(),
                );
            }
            if ui.button("↻").on_hover_text("重新扫描用户笔刷目录").clicked() {
                self.rescan();
            }
        });

        let mut need_rebuild = false;
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("搜索笔刷…")
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            need_rebuild = true;
        }

        // ---- 分组过滤 ----
        let groups = self.library.groups();
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(self.group.is_none(), format!("全部 {}", self.library.len()))
                .clicked()
            {
                self.group = None;
                need_rebuild = true;
            }
            for g in &groups {
                let on = self.group.as_deref() == Some(g.id.as_str());
                if ui
                    .selectable_label(on, format!("{} {}", g.display, g.count))
                    .clicked()
                {
                    self.group = Some(g.id.clone());
                    need_rebuild = true;
                }
            }
        });

        if need_rebuild {
            self.rebuild_visible();
        }

        ui.separator();
        let total = self.visible.len();
        ui.label(
            egui::RichText::new(format!("{total} 支匹配"))
                .weak()
                .small(),
        );

        // ---- 虚拟滚动列表（只为可见行渲染缩略图）----
        let row_h = ROW_H;
        let show_group_tag = self.group.is_none();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, row_h, total, |ui, range| {
                for pos in range {
                    let idx = self.visible[pos];
                    // 先把展示所需数据拷出来，避免与缩略图渲染的可变借用冲突
                    let Some((display, group_display, key, notes, embedded)) =
                        self.library.entries.get(idx).map(|e| {
                            (
                                e.display.clone(),
                                e.group_display.clone(),
                                e.key.clone(),
                                e.notes.clone(),
                                e.is_embedded(),
                            )
                        })
                    else {
                        continue;
                    };
                    let tex = self.thumbnail(ui.ctx(), idx, color);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_h),
                        egui::Sense::click(),
                    );
                    let selected = idx == self.selected;

                    // 行底色
                    let bg = if selected {
                        ui.visuals().selection.bg_fill
                    } else if resp.hovered() {
                        ui.visuals().widgets.hovered.bg_fill
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    if bg != egui::Color32::TRANSPARENT {
                        ui.painter()
                            .rect_filled(rect.shrink2(egui::vec2(0.0, 1.0)), 3.0, bg);
                    }

                    // 缩略图（保持 PREVIEW 宽高比）
                    let th = THUMB_W / thumb::ASPECT;
                    let img_rect = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + 2.0, rect.center().y - th * 0.5),
                        egui::vec2(THUMB_W, th),
                    );
                    ui.painter().image(
                        tex.id(),
                        img_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );

                    let text_color = if selected {
                        ui.visuals().strong_text_color()
                    } else if show_group_tag {
                        ui.visuals().weak_text_color()
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.painter().text(
                        egui::pos2(img_rect.max.x + 6.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        display,
                        egui::FontId::proportional(12.0),
                        text_color,
                    );
                    // 右侧弱化显示分组（仅"全部"视图下有意义）
                    if show_group_tag {
                        ui.painter().text(
                            egui::pos2(rect.max.x - 4.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            group_display,
                            egui::FontId::proportional(10.0),
                            ui.visuals().weak_text_color(),
                        );
                    }

                    let resp = resp.on_hover_text(format!(
                        "{key}\n{origin}{notes}",
                        origin = if embedded {
                            "内嵌官方包 (CC0)"
                        } else {
                            "用户目录"
                        },
                        notes = if notes.is_empty() {
                            String::new()
                        } else {
                            format!("\n\n{notes}")
                        }
                    ));
                    if resp.clicked() && idx != self.selected {
                        self.selected = idx;
                        changed = true;
                    }
                }
            });

        changed
    }

    /// 取得（必要时渲染并缓存）某支笔刷的缩略图。
    fn thumbnail(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        color: [f32; 3],
    ) -> egui::TextureHandle {
        if let Some(t) = self.thumbnails.get(&idx) {
            return t.clone();
        }
        let (w, h) = (thumb::PREVIEW_W, thumb::PREVIEW_H);
        let Some(entry) = self.library.entries.get(idx) else {
            return empty_texture(ctx, w, h);
        };
        let (key, rgba) = match entry.load() {
            Ok(b) => (entry.key.clone(), thumb::render(&b, color, w, h)),
            Err(_) => (entry.key.clone(), vec![0u8; w * h * 4]),
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
        let tex = ctx.load_texture(
            format!("thumb:{key}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.thumbnails.insert(idx, tex.clone());
        tex
    }

    /// 重新扫描用户笔刷目录（丢弃缓存，回到内嵌官方包 + 用户目录的合并结果）。
    fn rescan(&mut self) {
        let color = self.thumb_color;
        let selected_key = self.selected_entry().map(|e| e.key.clone());
        self.library = BrushLibrary::load();
        self.thumbnails.clear();
        // 置为不可能相等的值，强制下一帧按新库重绘缩略图
        self.thumb_color = Some([f32::NAN, f32::NAN, f32::NAN]);
        let _ = color;
        if let Some(k) = selected_key {
            if let Some(i) = self.library.find(&k) {
                self.selected = i;
            }
        }
        self.rebuild_visible();
    }
}

/// 一张全透明纹理（笔刷解析失败时的占位）。
fn empty_texture(ctx: &egui::Context, w: usize, h: usize) -> egui::TextureHandle {
    ctx.load_texture(
        "thumb-empty",
        egui::ColorImage::from_rgba_unmultiplied([w, h], &vec![0u8; w * h * 4]),
        egui::TextureOptions::LINEAR,
    )
}
