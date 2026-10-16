//! 笔刷面板 —— 照搬 MyPaint 的笔刷列表。
//!
//! 源码对照：
//!
//! | 本文件 | MyPaint |
//! |---|---|
//! | [`ICON_SIZE`] / [`cell`] | `BrushList.ICON_SIZE = 48`（`brushselectionwindow.py:66`） |
//! | [`total_border`] | `PixbufList.update`（`pixbuflist.py:243`） |
//! | [`grid_geometry`] | 同上（`tiles_w = width // total_w`，`tiles_h = ceil(n / tiles_w)`） |
//! | [`index_at`] | `PixbufList.index`（`pixbuflist.py:308`） |
//! | [`stroke_selection`] | `PixbufList.draw_cb`（`pixbuflist.py:373`） |
//! | [`thumb_for`] | `managedbrush_pixbuffunc` + `helpers.pixbuf_thumbnail`（`lib/helpers.py:454`） |
//! | `tooltips` | `managedbrush_namefunc`（`brushselectionwindow.py:46`） |
//!
//! MyPaint 的形态是 **48px 流式图标网格**：`BrushList` 是 `PixbufList` 的子类，
//! 每个分组一个 `BrushGroupTool` 停靠面板，面板里是这个网格。
//!
//! 两处**技术栈适配**（视觉结果一致，只是实现手段不同）：
//!
//! 1. **按需建纹理**：`PixbufList.update()` 会为列表里**每一项**立刻取 pixbuf
//!    （196 张 128×128 常驻 RAM）。我们改成只给滚动视口内的项建 GPU 纹理，
//!    否则 egui 里会一次性炸出 196 个纹理绑定。
//! 2. **搜索框与「全部」过滤**：MyPaint 没有（它靠分组面板 + `Brush Groups` 菜单）。
//!    这是本项目的**新增**入口，不是替换。
//!
//! 权限：官方包图标为 CC0，见 `assets/brushes/COPYING`。

use std::collections::HashMap;

use eframe::egui;
use pigment_engine::brushlib::{BrushEntry, BrushLibrary};
use pigment_engine::hokusai::Brush;

use crate::thumb;

/// 图标边长 —— `BrushList.ICON_SIZE`（`gui/brushselectionwindow.py:66`）。
pub const ICON_SIZE: f32 = 48.0;
/// 选中框线宽 —— `PixbufList.border_visible`（`gui/pixbuflist.py:69`）。
const BORDER_VISIBLE: f32 = 2.0;
/// 格内留白 —— `PixbufList.spacing_inside`。
const SPACING_INSIDE: f32 = 0.0;
/// 格外留白 —— `PixbufList.spacing_outside`。
const SPACING_OUTSIDE: f32 = 0.0;
/// 选中框外扩 —— `PixbufList.border_visible_outside_cell`。
const BORDER_VISIBLE_OUTSIDE_CELL: f32 = 1.0;

/// MyPaint 的启动默认笔刷 —— 对齐 `gui/brushmanager.py` 的 `_DEFAULT_BRUSH`。
pub const DEFAULT_BRUSH: &str = "Dieterle/Fan#1";
/// MyPaint 的默认橡皮 —— 对齐 `_DEFAULT_ERASER`。
///
/// 官方包实际目录名是全小写 `deevad/`，order.conf 里的分组展示名才是 `Deevad`。
#[allow(dead_code)]
pub const DEFAULT_ERASER: &str = "deevad/kneaded_eraser_large";
/// `DEFAULT_BRUSH` 缺失时的兜底（官方包被裁剪等情况）。
const FALLBACK_BRUSH: &str = "classic/dry_brush";

/// `PixbufList.update()` 的 `total_border`（`gui/pixbuflist.py:243`）。
fn total_border() -> f32 {
    BORDER_VISIBLE + SPACING_INSIDE + SPACING_OUTSIDE
}

/// 一个格子的边长 —— 同函数的 `total_w` / `total_h`。
fn cell() -> f32 {
    ICON_SIZE + 2.0 * total_border()
}

/// 网格几何 —— `PixbufList.update()` 的 `tiles_w` / `tiles_h`。
///
/// 返回 `(每行格数, 总行数)`。
fn grid_geometry(width: f32, count: usize) -> (usize, usize) {
    let c = cell();
    let tiles_w = ((width / c) as usize).max(1);
    let tiles_h = count.div_ceil(tiles_w).max(1);
    (tiles_w, tiles_h)
}

/// 命中测试 —— `PixbufList.index`（`gui/pixbuflist.py:308`）。
///
/// 坐标为**内容坐标**（相对网格左上角）。超出右边界时夹到最后一列，
/// 与 MyPaint 一致（点空白区域落在最近的格子上）。
fn index_at(x: f32, y: f32, tiles_w: usize, count: usize) -> usize {
    if count == 0 || tiles_w == 0 {
        return 0;
    }
    let c = cell();
    let mut col = (x / c).floor() as i64;
    col = col.clamp(0, tiles_w as i64 - 1);
    let row = (y / c).floor() as i64;
    let i = (col + tiles_w as i64 * row).max(0) as usize;
    i.min(count - 1)
}

pub struct BrushPanel {
    pub library: BrushLibrary,
    /// 当前选中项在 `library.entries` 中的下标。
    pub selected: usize,
    /// 名称搜索关键字（本项目的**新增**入口，MyPaint 无此控件）。
    pub search: String,
    /// 分组过滤（`None` = 全部）。
    pub group: Option<String>,
    /// 缩略图缓存：library 下标 → 纹理（扮演 `PixbufList.thumbnails` 的角色）。
    thumbnails: HashMap<usize, egui::TextureHandle>,
    /// 过滤后的 library 下标列表。
    visible: Vec<usize>,
    /// 与 [`Self::visible`] 一一对应的 tooltip 文本（`managedbrush_namefunc` 的结果）。
    ///
    /// 在 [`Self::rebuild_visible`] 里一并算好：tooltip 需要闭包读它，
    /// 提前算好就不必在绘制闭包里再借 `&self`。
    tooltips: Vec<String>,
    /// 与 [`Self::visible`] 一一对应的 `(笔刷名, description)`。
    labels: Vec<(String, String)>,
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
            visible: Vec::new(),
            tooltips: Vec::new(),
            labels: Vec::new(),
        };
        p.rebuild_visible();
        p
    }

    pub fn selected_entry(&self) -> Option<&BrushEntry> {
        self.library.entries.get(self.selected)
    }

    /// 某支笔刷的 48×48 图标（与我方笔刷列表用的是同一张，含缓存）。
    ///
    /// 供笔刷编辑器顶部复用 —— MyPaint 那里也是直接取 `ManagedBrush.preview`。
    pub fn icon(&mut self, ctx: &egui::Context, idx: usize) -> egui::TextureHandle {
        thumb_for(ctx, &self.library, &mut self.thumbnails, idx)
    }

    /// 解析当前选中笔刷（解析失败极少见，调用方以兜底笔刷处理）。
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
        self.rebuild_labels();
    }

    /// 预生成列表标签与 tooltip —— `managedbrush_namefunc`（`brushselectionwindow.py:46`）。
    ///
    /// tooltip 模板：`"{name}\n{description}"`（description 为空时只有 name）。
    /// 我们额外补一行路径与来源（MyPaint 的 tooltip 只有两行，这是新增信息）。
    fn rebuild_labels(&mut self) {
        self.labels.clear();
        self.tooltips.clear();
        for &i in &self.visible {
            let Some(e) = self.library.entries.get(i) else {
                continue;
            };
            let name = e.mypaint_name();
            let mut tip = name.clone();
            if !e.display.is_empty() {
                tip.push('\n');
                tip.push_str(&e.display);
            }
            tip.push_str(&format!(
                "\n{}\n{}",
                e.key,
                if e.is_embedded() {
                    "内嵌官方包 (CC0)"
                } else {
                    "用户目录"
                }
            ));
            self.labels.push((name, e.display.clone()));
            self.tooltips.push(tip);
        }
    }

    /// 绘制面板。返回 `true` 表示**选中笔刷发生变化**（调用方需重载笔刷）。
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
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

        // ---- 分组过滤（本项目入口；MyPaint 用独立的 BrushGroupTool 面板）----
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
        ui.label(
            egui::RichText::new(format!("{} 支", self.visible.len()))
                .weak()
                .small(),
        );

        self.grid(ui)
    }

    /// 48px 流式图标网格 —— `PixbufList`。
    fn grid(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let count = self.visible.len();
        if count == 0 {
            ui.label(egui::RichText::new("没有匹配的笔刷").weak());
            return false;
        }
        let (tiles_w, tiles_h) = grid_geometry(ui.available_width(), count);
        let c = cell();
        let total_size = egui::vec2(tiles_w as f32 * c, tiles_h as f32 * c);

        let visuals = ui.visuals().clone();
        let bg = visuals.extreme_bg_color;
        let selected_fill = visuals.selection.bg_fill;
        let hover_fill = visuals.widgets.hovered.bg_fill;

        let mut clicked_idx: Option<usize> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let (rect, response) =
                    ui.allocate_exact_size(total_size, egui::Sense::click_and_drag());
                let painter = ui.painter_at(rect);
                // `PixbufList.draw_cb` 先铺主题背景
                painter.rect_filled(rect, egui::CornerRadius::ZERO, bg);

                // 只画滚动视口覆盖到的行（技术栈适配：避免一次性建 196 个纹理）
                let first_row = (viewport.min.y / c).floor().max(0.0) as usize;
                let last_row = (((viewport.max.y / c).ceil() as usize) + 1).min(tiles_h);

                let hover_pos = response.hover_pos().filter(|p| rect.contains(*p));
                let mut hovered: Option<usize> = None;

                for row in first_row..last_row {
                    for col in 0..tiles_w {
                        let slot = row * tiles_w + col;
                        if slot >= count {
                            break;
                        }
                        let idx = self.visible[slot];
                        let origin = rect.min + egui::vec2(col as f32 * c, row as f32 * c);
                        let cell_rect = egui::Rect::from_min_size(origin, egui::vec2(c, c));
                        let icon_rect = egui::Rect::from_min_size(
                            origin + egui::vec2(total_border(), total_border()),
                            egui::vec2(ICON_SIZE, ICON_SIZE),
                        );

                        let is_hovered = hover_pos.is_some_and(|p| cell_rect.contains(p));
                        if is_hovered {
                            hovered = Some(slot);
                        }

                        if idx == self.selected {
                            stroke_selection(&painter, cell_rect, selected_fill);
                        } else if is_hovered {
                            stroke_selection(&painter, cell_rect, hover_fill);
                        }

                        let tex = thumb_for(ui.ctx(), &self.library, &mut self.thumbnails, idx);
                        painter.image(
                            tex.id(),
                            icon_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }

                // ---- tooltip（`managedbrush_namefunc`）----
                let response = match hovered.and_then(|slot| self.tooltips.get(slot)) {
                    Some(tip) => response.on_hover_text(tip.clone()),
                    None => response,
                };
                let _ = &self.labels;

                // ---- 点击选中：照搬 `button_press_cb`（左键选中；右键菜单留待后续）----
                if response.clicked() {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let local = pos - rect.min;
                        let slot = index_at(local.x, local.y, tiles_w, count);
                        clicked_idx = self.visible.get(slot).copied();
                    }
                }
            });

        if let Some(idx) = clicked_idx {
            if idx != self.selected {
                self.selected = idx;
                changed = true;
            }
        }
        changed
    }

    /// 重新扫描用户笔刷目录（丢弃缓存，回到内嵌官方包 + 用户目录的合并结果）。
    fn rescan(&mut self) {
        let selected_key = self.selected_entry().map(|e| e.key.clone());
        self.library = BrushLibrary::load();
        self.thumbnails.clear();
        if let Some(k) = selected_key {
            if let Some(i) = self.library.find(&k) {
                self.selected = i;
            }
        }
        self.rebuild_visible();
    }
}

/// 取（必要时解码并缓存）某支笔刷的 48×48 图标。
///
/// 照搬 `BrushList` 的取图链：`managedbrush_pixbuffunc` → `ManagedBrush.preview`
/// → 优先随附 `_prev.png`、否则实时渲染（[`thumb::render`]）；
/// 再经 `helpers.pixbuf_thumbnail(pixbuf, 48, 48)` 等比缩小并居中到白底。
fn thumb_for(
    ctx: &egui::Context,
    library: &BrushLibrary,
    cache: &mut HashMap<usize, egui::TextureHandle>,
    idx: usize,
) -> egui::TextureHandle {
    if let Some(t) = cache.get(&idx) {
        return t.clone();
    }
    let Some(entry) = library.entries.get(idx) else {
        return ctx.load_texture(
            "thumb-missing",
            icon_image(&white_icon()),
            egui::TextureOptions::LINEAR,
        );
    };
    let rgba = match entry.preview_png() {
        // `ManagedBrush._load_preview()`：读随附的 128×128 预览图
        Some(bytes) => decode_png(&bytes),
        // 回退：`drawutils.render_brush_preview_pixbuf`
        None => entry.load().ok().map(|b| thumb::render(&b)),
    };
    let (key, rgba) = match rgba {
        Some(src) => (
            entry.key.clone(),
            thumbnail_on_white(&src, thumb::PREVIEW_W, thumb::PREVIEW_H, ICON_SIZE as usize, ICON_SIZE as usize),
        ),
        None => (format!("{}-broken", entry.key), white_icon()),
    };
    let tex = ctx.load_texture(
        format!("thumb:{key}"),
        icon_image(&rgba),
        egui::TextureOptions::LINEAR,
    );
    cache.insert(idx, tex.clone());
    tex
}

fn icon_image(rgba: &[u8]) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([ICON_SIZE as usize, ICON_SIZE as usize], rgba)
}

/// 选中/悬停描边 —— `PixbufList.draw_cb`（`gui/pixbuflist.py:373`）。
///
/// 原算法：先把格子按 `spacing_outside` 内缩，再按
/// `border_visible_outside_cell` 外扩一格，然后描
/// `border_visible + border_visible_outside_cell` 层、逐层内缩 1px 的边框。
fn stroke_selection(painter: &egui::Painter, cell_rect: egui::Rect, color: egui::Color32) {
    let shrink = |r: egui::Rect, p: f32| -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(r.min.x + p, r.min.y + p),
            egui::pos2(r.max.x - p, r.max.y - p),
        )
    };
    let mut r = shrink(cell_rect, SPACING_OUTSIDE);
    for _ in 0..BORDER_VISIBLE_OUTSIDE_CELL as usize {
        r = shrink(r, -1.0);
    }
    let layers = (BORDER_VISIBLE + BORDER_VISIBLE_OUTSIDE_CELL) as usize;
    for _ in 0..layers {
        painter.rect_stroke(
            egui::Rect::from_min_max(r.min, egui::pos2(r.max.x - 1.0, r.max.y - 1.0)),
            egui::CornerRadius::ZERO,
            egui::Stroke::new(1.0, color),
            egui::StrokeKind::Middle,
        );
        r = shrink(r, 1.0);
    }
}

/// 解码 `_prev.png` → 128×128 的 RGBA8，失败返回 `None`。
fn decode_png(bytes: &[u8]) -> Option<Vec<u8>> {
    // MyPaint 的 `_prev.png` 有 indexed / gray / RGB / RGBA 四种色型，
    // 统一归到 RGBA8（`get_preview()` 拿到的是带 alpha 的 pixbuf）。
    let img = image::load_from_memory(bytes).ok()?;
    Some(img.to_rgba8().into_raw())
}

/// `helpers.pixbuf_thumbnail(src, w, h)`（`lib/helpers.py:454`）：
/// `scale_proportionally`（只缩不放）后**居中合成到白底**。
fn thumbnail_on_white(src: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Vec<u8> {
    let mut dst = vec![255u8; w * h * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return dst;
    }
    let scale = (w as f32 / sw as f32).min(h as f32 / sh as f32);
    // `shrink_only=True`：scale >= 1 时原样使用
    let (nw, nh) = if scale >= 1.0 {
        (sw, sh)
    } else {
        (
            ((sw as f32 * scale) as usize).max(1),
            ((sh as f32 * scale) as usize).max(1),
        )
    };
    let scaled: Vec<u8> = if (nw, nh) == (sw, sh) {
        src[..sw * sh * 4].to_vec()
    } else {
        let buf = image::RgbaImage::from_raw(sw as u32, sh as u32, src.to_vec())
            .expect("源图尺寸与缓冲不符");
        image::imageops::resize(
            &buf,
            nw as u32,
            nh as u32,
            image::imageops::FilterType::Triangle,
        )
        .into_raw()
    };
    let ox = (w - nw) / 2;
    let oy = (h - nh) / 2;
    for y in 0..nh {
        for x in 0..nw {
            let si = (y * nw + x) * 4;
            let a = scaled[si + 3] as u32;
            let di = ((y + oy) * w + (x + ox)) * 4;
            // 源图带 alpha → 与白底合成（`src2.composite(dst, ...)`）
            for ch in 0..3 {
                dst[di + ch] = ((scaled[si + ch] as u32 * a + 255 * (255 - a)) / 255) as u8;
            }
            dst[di + 3] = 255;
        }
    }
    dst
}

/// 全白图标（解析失败时的占位）。
fn white_icon() -> Vec<u8> {
    vec![255u8; ICON_SIZE as usize * ICON_SIZE as usize * 4]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `grid_geometry` 照搬 `PixbufList.update`：240px 宽 / 52px 格 → 4 列。
    #[test]
    fn grid_geometry_matches_pixbuflist() {
        assert_eq!(grid_geometry(240.0, 196), (4, 49));
        assert_eq!(grid_geometry(52.0, 10), (1, 10));
        // 宽度不足一格时至少 1 列
        assert_eq!(grid_geometry(10.0, 3), (1, 3));
        // ceil 的边界
        assert_eq!(grid_geometry(208.0, 8), (4, 2));
        assert_eq!(grid_geometry(208.0, 9), (4, 3));
    }

    /// `index_at` 照搬 `PixbufList.index`：超出右边界夹到最后一列。
    #[test]
    fn index_at_clamps_to_last_column() {
        let (tw, c) = (4usize, cell());
        assert_eq!(index_at(0.0, 0.0, tw, 10), 0);
        assert_eq!(index_at(c * 3.5, 0.0, tw, 10), 3);
        assert_eq!(index_at(5.0, c * 1.5, tw, 10), 4);
        // 负坐标夹到 0
        assert_eq!(index_at(-30.0, -30.0, tw, 10), 0);
        // 末尾越界夹到 count-1
        assert_eq!(index_at(c * 3.5, c * 9.5, tw, 7), 6);
    }

    #[test]
    fn cell_is_52px_like_mypaint() {
        assert_eq!(cell(), 52.0);
        assert_eq!(ICON_SIZE, 48.0);
    }

    /// `pixbuf_thumbnail` 的等比缩小：128×128 → 48×48 铺满整格。
    #[test]
    fn thumbnail_fills_square_cell() {
        let mut src = vec![0u8; 128 * 128 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 10;
            px[1] = 10;
            px[2] = 10;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 128, 128, 48, 48);
        assert_eq!(out.len(), 48 * 48 * 4);
        assert_eq!(&out[0..3], &[10, 10, 10]);
        assert_eq!(out[3], 255);
    }

    /// 半透明源图与白底合成（`src2.composite(dst, ...)`）。
    #[test]
    fn thumbnail_composites_alpha_on_white() {
        // 全透明 → 输出应全是白底
        let src = vec![0u8; 128 * 128 * 4];
        let out = thumbnail_on_white(&src, 128, 128, 48, 48);
        assert!(out.iter().all(|&b| b == 255), "全透明源图应为纯白");
    }

    /// 非正方形源图：等比缩放后居中，上下留白为白底。
    #[test]
    fn thumbnail_centers_non_square() {
        let mut src = vec![0u8; 128 * 64 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 200;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 128, 64, 48, 48);
        assert_eq!(&out[0..3], &[255, 255, 255], "首行应为白底留白");
        let mid = (24 * 48 + 24) * 4;
        assert_eq!(out[mid], 200);
    }

    /// 「只缩不放」：源图比目标小则原样居中。
    #[test]
    fn thumbnail_never_upscales() {
        let mut src = vec![0u8; 16 * 16 * 4];
        for px in src.chunks_mut(4) {
            px[0] = 77;
            px[3] = 255;
        }
        let out = thumbnail_on_white(&src, 16, 16, 48, 48);
        assert_eq!(out.len(), 48 * 48 * 4);
        let mid = (24 * 48 + 24) * 4;
        assert_eq!(out[mid], 77);
        assert_eq!(&out[0..3], &[255, 255, 255]);
    }

    /// 官方包 196 支全部自带 `_prev.png`，且都能解码成 128×128。
    #[test]
    fn every_embedded_brush_has_decodable_preview() {
        let lib = BrushLibrary::embedded();
        assert_eq!(lib.len(), 196);
        let mut bad = Vec::new();
        for e in &lib.entries {
            match e.preview_png().and_then(|b| decode_png(&b)) {
                Some(px) => assert_eq!(px.len(), 128 * 128 * 4, "{} 尺寸不对", e.key),
                None => bad.push(e.key.clone()),
            }
        }
        assert!(bad.is_empty(), "缺少/无法解码预览图: {bad:?}");
    }

    /// 每个条目都有 tooltip，且首行是笔刷名（`managedbrush_namefunc` 的模板）。
    #[test]
    fn tooltips_follow_mypaint_template() {
        let p = BrushPanel::new();
        assert_eq!(p.tooltips.len(), p.visible.len());
        assert_eq!(p.labels.len(), p.visible.len());
        for (slot, &idx) in p.visible.iter().enumerate() {
            let e = &p.library.entries[idx];
            let first_line = p.tooltips[slot].lines().next().unwrap_or_default();
            assert_eq!(first_line, e.mypaint_name());
        }
    }
}
