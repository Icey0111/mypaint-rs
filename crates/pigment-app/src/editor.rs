//! 笔刷编辑器 —— MyPaint 的灵魂 UI：**输入 → 曲线 → 设置** 三层结构。
//!
//! 曲线求值直接调用 [`InputMapping::eval`]，与引擎运行时是同一条代码路径，
//! 所以「编辑器里看到的曲线」就是「真正画出来的曲线」，不存在预览与实现的漂移。
//!
//! 交互（对齐 MyPaint 的笔刷编辑器）：
//! - 左栏点设置 → 右侧编辑该设置的 base 值与各输入曲线
//! - 曲线上拖动控制点 = 改曲线
//! - 曲线空白处按下 = 在该处**插入**控制点并开始拖动
//! - 右键控制点 = 删除
//! - x 轴保持升序（拖拽时被相邻控制点夹住），与 libmypaint 的曲线约束一致
//!
//! 设置列表按 **MyPaint 的 11 个命名分组**呈现（成员表照抄
//! `mypaint/gui/brusheditor.py` 的 `groups` 定义），`color_h/s/v` 与 MyPaint
//! 一样不显示——它们由颜色 UI 管理，不该出现在笔刷编辑器里。

use eframe::egui;
use pigment_engine::hokusai::{Brush, BrushInput, BrushSetting, InputMapping, SettingValue};

/// 曲线编辑区高度。
const CURVE_H: f32 = 170.0;
/// 控制点命中半径（像素）。
const HIT_R: f32 = 11.0;

/// MyPaint 笔刷编辑器的分组结构。
///
/// 逐项照抄 `mypaint/gui/brusheditor.py:331-497` 的 `groups` 字面量（含顺序）。
/// 唯一缺项是 `smudge_bucket` —— hokusai 的 `BrushSetting` 枚举里没有它
/// （见《MyPaint源码逐项比对报告》第 1.1 节），所以这里无法列出。
const MP_GROUPS: &[(&str, &[BrushSetting])] = &[
    (
        "Basic",
        &[
            BrushSetting::Radius,
            BrushSetting::RadiusByRandom,
            BrushSetting::Hardness,
            BrushSetting::SnapToPixel,
            BrushSetting::AntiAliasing,
            BrushSetting::Eraser,
            BrushSetting::OffsetByRandom,
            BrushSetting::EllipticalDabAngle,
            BrushSetting::EllipticalDabRatio,
            BrushSetting::DirectionFilter,
            BrushSetting::PressureGainLog,
        ],
    ),
    (
        "Opacity",
        &[
            BrushSetting::Opaque,
            BrushSetting::OpaqueMultiply,
            BrushSetting::OpaqueLinearize,
            BrushSetting::LockAlpha,
        ],
    ),
    (
        "Dabs",
        &[
            BrushSetting::DabsPerBasicRadius,
            BrushSetting::DabsPerActualRadius,
            BrushSetting::DabsPerSecond,
        ],
    ),
    (
        "Smudge",
        &[
            BrushSetting::Smudge,
            BrushSetting::SmudgeLength,
            BrushSetting::SmudgeLengthLog,
            BrushSetting::SmudgeRadiusLog,
            BrushSetting::SmudgeTransparency,
            // smudge_bucket 缺失（hokusai 未实现）
        ],
    ),
    (
        "Speed",
        &[
            BrushSetting::Speed1Slowness,
            BrushSetting::Speed2Slowness,
            BrushSetting::Speed1Gamma,
            BrushSetting::Speed2Gamma,
            BrushSetting::OffsetBySpeed,
            BrushSetting::OffsetBySpeedSlowness,
        ],
    ),
    (
        "Directional Offsets",
        &[
            BrushSetting::OffsetMultiplier,
            BrushSetting::OffsetAngleAdj,
            BrushSetting::OffsetX,
            BrushSetting::OffsetY,
            BrushSetting::OffsetAngle,
            BrushSetting::OffsetAngle2,
            BrushSetting::OffsetAngleAsc,
            BrushSetting::OffsetAngle2Asc,
            BrushSetting::OffsetAngleView,
            BrushSetting::OffsetAngle2View,
        ],
    ),
    (
        "Tracking",
        &[
            BrushSetting::SlowTracking,
            BrushSetting::SlowTrackingPerDab,
            BrushSetting::TrackingNoise,
        ],
    ),
    (
        "Stroke",
        &[
            BrushSetting::StrokeThreshold,
            BrushSetting::StrokeDurationLogarithmic,
            BrushSetting::StrokeHoldtime,
        ],
    ),
    (
        "Color",
        &[
            BrushSetting::Paint,
            BrushSetting::ChangeColorH,
            BrushSetting::ChangeColorL,
            BrushSetting::ChangeColorHslS,
            BrushSetting::ChangeColorV,
            BrushSetting::ChangeColorHsvS,
            BrushSetting::RestoreColor,
            BrushSetting::Colorize,
            BrushSetting::Posterize,
            BrushSetting::PosterizeNum,
        ],
    ),
    (
        "GridMap",
        &[
            BrushSetting::GridmapScale,
            BrushSetting::GridmapScaleX,
            BrushSetting::GridmapScaleY,
        ],
    ),
    (
        "Custom",
        &[BrushSetting::CustomInput, BrushSetting::CustomInputSlowness],
    ),
];

/// MyPaint 刻意从编辑器隐藏的设置（`brusheditor.py` 的 `hidden_settings`）。
///
/// 这三个由颜色 UI 写入，不是用户直接调的笔刷动态。我们的颜色面板负责它们。
const MP_HIDDEN: &[BrushSetting] = &[
    BrushSetting::ColorH,
    BrushSetting::ColorS,
    BrushSetting::ColorV,
];

/// 该设置是否仍处于「零值」态（用于「仅显示非默认设置」过滤）。
fn is_default(sv: &SettingValue) -> bool {
    sv.base_value == 0.0 && sv.inputs.is_empty() && sv.unknown_inputs.is_empty()
}

pub struct BrushEditor {
    pub setting: BrushSetting,
    pub only_nondefault: bool,
    /// 正在编辑曲线的输入下标（`SettingValue::inputs` 内）。
    curve: usize,
    /// 正在拖动的控制点下标。
    drag: Option<usize>,
}

impl Default for BrushEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl BrushEditor {
    pub fn new() -> Self {
        Self {
            setting: BrushSetting::Opaque,
            only_nondefault: true,
            curve: 0,
            drag: None,
        }
    }

    /// 绘制编辑器。`edit` 为被编辑的笔刷（改它）。返回 `true` 表示笔刷被修改。
    ///
    /// `icon` 是当前笔刷的预览图（与我方笔刷列表同一张），由调用方给出 ——
    /// 对齐 MyPaint `brusheditor.py:786`：编辑器顶部的图标就是
    /// `ManagedBrush.preview`（随附的 `_prev.png`），**不随设置改动实时重画**。
    pub fn show(&mut self, ui: &mut egui::Ui, edit: &mut Brush, icon: Option<&egui::TextureHandle>) -> bool {
        self.icon_section(ui, icon);
        ui.separator();

        // ---- 设置列表（按 MyPaint 的命名分组组织）----
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("设置").strong());
            ui.label(
                egui::RichText::new(format!("{} 组", MP_GROUPS.len()))
                    .weak()
                    .small(),
            );
        });
        ui.checkbox(&mut self.only_nondefault, "仅显示非默认设置")
            .on_hover_text(
                "分组与成员表照抄 MyPaint 的 gui/brusheditor.py；\n\
                 color_h / color_s / color_v 与 MyPaint 一样不在此列出（由颜色面板管理）",
            );

        // 未被任何分组的设置 → MyPaint 会归入 Experimental
        let ungrouped: Vec<BrushSetting> = BrushSetting::ALL
            .iter()
            .copied()
            .filter(|s| {
                !MP_HIDDEN.contains(s) && !MP_GROUPS.iter().any(|(_, m)| m.contains(s))
            })
            .collect();

        let mut pick: Option<BrushSetting> = None;
        egui::ScrollArea::vertical()
            .id_salt("brush-settings")
            .max_height(260.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (title, members) in MP_GROUPS {
                    if let Some(p) =
                        setting_group_ui(ui, edit, self.setting, self.only_nondefault, title, members)
                    {
                        pick = Some(p);
                    }
                }
                if !ungrouped.is_empty() {
                    if let Some(p) = setting_group_ui(
                        ui,
                        edit,
                        self.setting,
                        self.only_nondefault,
                        "Experimental",
                        &ungrouped,
                    ) {
                        pick = Some(p);
                    }
                }
            });
        if let Some(s) = pick {
            if s != self.setting {
                self.setting = s;
                self.curve = 0;
            }
        }

        ui.separator();
        let mut changed = false;

        // ---- 当前设置的 base 值 ----
        let setting = self.setting;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(setting.cname()).strong());
            let sv = edit.get_mut(setting);
            changed |= ui
                .add(
                    egui::DragValue::new(&mut sv.base_value)
                        .speed(0.01)
                        .range(-40.0..=40.0),
                )
                .changed();
            if ui.small_button("清零").clicked() {
                sv.base_value = 0.0;
                changed = true;
            }
            if ui.small_button("重置该设置").on_hover_text("清空 base 与全部输入曲线").clicked() {
                *sv = pigment_engine::hokusai::SettingValue::constant(0.0);
                changed = true;
            }
        });

        // ---- 输入曲线 ----
        let n_inputs = edit.get(setting).inputs.len();
        if self.curve >= n_inputs {
            self.curve = 0;
        }
        if n_inputs == 0 {
            ui.label(
                egui::RichText::new("该设置为常量：没有输入曲线，加一条即可让它随压感/速度变化")
                    .weak()
                    .small(),
            );
        } else {
            ui.label(egui::RichText::new("输入曲线").strong());
        }

        if n_inputs > 0 {
            egui::ScrollArea::horizontal()
                .id_salt("brush-inputs")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, m) in edit.get(setting).inputs.iter().enumerate() {
                            if ui
                                .selectable_label(self.curve == i, m.input.cname())
                                .clicked()
                            {
                                self.curve = i;
                            }
                        }
                    });
                });
        }

        let mut remove_curve = false;
        let mut add_linear = false;
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("add-input")
                .selected_text("＋ 输入")
                .width(130.0)
                .show_ui(ui, |ui| {
                    for inp in BrushInput::ALL {
                        let exists = edit
                            .get(setting)
                            .inputs
                            .iter()
                            .any(|m| m.input == *inp);
                        if exists {
                            continue;
                        }
                        if ui.selectable_label(false, inp.cname()).clicked() {
                            let sv = edit.get_mut(setting);
                            sv.inputs.push(InputMapping::new(*inp));
                            self.curve = sv.inputs.len() - 1;
                            changed = true;
                        }
                    }
                });
            if n_inputs > 0 {
                if ui.small_button("重置为线性").clicked() {
                    add_linear = true;
                }
                if ui.small_button("删除该曲线").clicked() {
                    remove_curve = true;
                }
            }
        });
        if add_linear {
            let sv = edit.get_mut(setting);
            sv.inputs[self.curve].points = vec![(0.0, 0.0), (1.0, 1.0)];
            changed = true;
        }
        if remove_curve {
            let sv = edit.get_mut(setting);
            sv.inputs.remove(self.curve);
            self.curve = self.curve.saturating_sub(1);
            changed = true;
        }

        // ---- 曲线画布 ----
        if edit.get(setting).inputs.len() > self.curve {
            changed |= self.curve_canvas(ui, edit);
        }

        changed
    }

    /// 编辑器顶部的笔刷图标 —— 照搬 `brusheditor.py:786`。
    ///
    /// MyPaint 在这里取 `ManagedBrush.preview` 并按控件尺寸
    /// `scale_simple(w, h, BILINEAR)` 缩放；它是随附的 `_prev.png`，
    /// **不随设置改动重画**。取不到就留空（MyPaint 则显示一张透明 pixbuf）。
    fn icon_section(&mut self, ui: &mut egui::Ui, icon: Option<&egui::TextureHandle>) {
        let w = ui.available_width().max(64.0);
        match icon {
            Some(t) => {
                ui.image((t.id(), egui::vec2(w, w)));
            }
            None => {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(w, w), egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, egui::CornerRadius::ZERO, ui.visuals().extreme_bg_color);
            }
        }
        ui.label(
            egui::RichText::new("笔刷图标：随附的 128×128 预览图（MyPaint 同上，不随设置改动重画）")
                .weak()
                .small(),
        );
    }

    /// 曲线编辑器主体。
    fn curve_canvas(&mut self, ui: &mut egui::Ui, edit: &mut Brush) -> bool {
        let mut changed = false;
        let setting = self.setting;
        let ci = self.curve;

        // 取一份快照用于绘制（避开 &mut 借用冲突）
        let mapping = edit.get(setting).inputs[ci].clone();
        let mut xmax = 1.0f32;
        let mut ymax = 1.0f32;
        for (x, y) in &mapping.points {
            xmax = xmax.max(*x);
            ymax = ymax.max(y.abs());
        }
        let xmax = nice_ceil(xmax);
        let ymax = nice_ceil(ymax);

        let w = ui.available_width();
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(w, CURVE_H), egui::Sense::click_and_drag());
        let plot = rect.shrink(8.0);
        let sx = |x: f32| plot.left() + (x / xmax) * plot.width();
        let sy = |y: f32| plot.center().y - (y / ymax) * plot.height() * 0.5;
        let inv_x = |px: f32| ((px - plot.left()) / plot.width()) * xmax;
        let inv_y = |py: f32| ((plot.center().y - py) / (plot.height() * 0.5)) * ymax;

        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
        painter.rect_stroke(
            rect,
            4.0,
            visuals.widgets.noninteractive.bg_stroke,
            egui::StrokeKind::Inside,
        );

        // 网格
        let grid = visuals.weak_text_color().gamma_multiply(0.35);
        for i in 4..8 {
            if i % 2 == 1 {
                continue;
            }
            let x = xmax * i as f32 / 8.0;
            painter.line_segment(
                [egui::pos2(sx(x), plot.top()), egui::pos2(sx(x), plot.bottom())],
                egui::Stroke::new(1.0, grid),
            );
        }
        for k in 0..=2 {
            let y = ymax * k as f32 / 2.0;
            for sign in [-1.0, 1.0] {
                let py = sy(y * sign);
                painter.line_segment(
                    [egui::pos2(plot.left(), py), egui::pos2(plot.right(), py)],
                    egui::Stroke::new(1.0, grid),
                );
            }
        }
        // 坐标轴
        let axis = visuals.weak_text_color();
        painter.line_segment(
            [
                egui::pos2(plot.left(), sy(0.0)),
                egui::pos2(plot.right(), sy(0.0)),
            ],
            egui::Stroke::new(1.2, axis),
        );
        painter.line_segment(
            [egui::pos2(sx(0.0), plot.top()), egui::pos2(sx(0.0), plot.bottom())],
            egui::Stroke::new(1.2, axis),
        );

        // ---- 交互：命中检测 ----
        let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
        let hit = pointer.and_then(|p| nearest(&mapping, p, &sx, &sy));

        if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos() {
                match hit {
                    Some(i) => self.drag = Some(i),
                    None => {
                        // 空白处按下 = 插入控制点（保持 x 升序）
                        let x = inv_x(p.x).clamp(0.0, xmax);
                        let y = mapping.eval(x).clamp(-ymax, ymax);
                        let at = insert_point(
                            &mut edit.get_mut(setting).inputs[ci].points,
                            x,
                            y,
                        );
                        self.drag = Some(at);
                        changed = true;
                    }
                }
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary) {
            if let (Some(i), Some(p)) = (self.drag, resp.interact_pointer_pos()) {
                changed |= move_point(
                    &mut edit.get_mut(setting).inputs[ci].points,
                    i,
                    inv_x(p.x),
                    inv_y(p.y),
                    xmax,
                    ymax,
                );
            }
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary) {
            self.drag = None;
        }
        if resp.clicked_by(egui::PointerButton::Secondary) {
            if let Some(i) = hit {
                edit.get_mut(setting).inputs[ci].points.remove(i);
                self.drag = None;
                changed = true;
            }
        }

        // ---- 绘制曲线（用当前真实数据）----
        let live = &edit.get(setting).inputs[ci];
        let curve_pts: Vec<egui::Pos2> = (0..=96)
            .map(|i| {
                let x = xmax * i as f32 / 96.0;
                egui::pos2(sx(x), sy(live.eval(x)))
            })
            .collect();
        let accent = visuals.selection.bg_fill;
        painter.add(egui::Shape::line(
            curve_pts,
            egui::Stroke::new(1.8, accent),
        ));

        // ---- 绘制控制点 ----
        let point_color = visuals.strong_text_color();
        let dark = visuals.extreme_bg_color;
        for (i, &(x, y)) in live.points.iter().enumerate() {
            let c = egui::pos2(sx(x), sy(y));
            let active = self.drag == Some(i) || hit == Some(i);
            painter.circle_filled(c, if active { 5.0 } else { 3.5 }, point_color);
            painter.circle_stroke(
                c,
                if active { 6.5 } else { 5.0 },
                egui::Stroke::new(1.0, dark),
            );
        }

        // ---- 读数 ----
        let font = egui::FontId::proportional(10.0);
        let weak = visuals.weak_text_color();
        painter.text(
            plot.left_top() + egui::vec2(2.0, 2.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{} × {}   x∈[0,{xmax:.1}]  y∈[±{ymax:.1}]",
                setting.cname(),
                live.input.cname()
            ),
            font.clone(),
            weak,
        );
        let base = edit.get(setting).base_value;
        painter.text(
            plot.left_bottom() + egui::vec2(2.0, -2.0),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "base {:+.3}   曲线 {:.3} (x=1)   → 结果 {:+.3}   [左键拖点/空白加点 · 右键删点]",
                base,
                live.eval(1.0),
                base + live.eval(1.0)
            ),
            font.clone(),
            weak,
        );
        if let Some(i) = self.drag.or(hit) {
            if let Some(&(x, y)) = live.points.get(i) {
                painter.text(
                    plot.right_top() + egui::vec2(-2.0, 2.0),
                    egui::Align2::RIGHT_TOP,
                    format!("#{i}  ({x:.3}, {y:+.3})"),
                    font,
                    point_color,
                );
            }
        }
        let _ = resp;

        changed
    }
}

/// 渲染 MyPaint 分组列表中的一个分组：小标题 + 缩进的成员行。
///
/// 「仅显示非默认设置」打开时，全为默认值的组会整体隐藏，避免列表变成长串空组。
fn setting_group_ui(
    ui: &mut egui::Ui,
    edit: &Brush,
    current: BrushSetting,
    only_nondefault: bool,
    title: &str,
    members: &[BrushSetting],
) -> Option<BrushSetting> {
    let shown: Vec<BrushSetting> = members
        .iter()
        .copied()
        .filter(|s| !only_nondefault || !is_default(edit.get(*s)))
        .collect();
    if shown.is_empty() {
        return None;
    }
    ui.add_space(2.0);
    ui.label(egui::RichText::new(title).strong().small());
    let mut picked = None;
    for s in shown {
        let sv = edit.get(s);
        let text = format!(
            "{:<24}{:>7.2}  ×{}",
            s.cname(),
            sv.base_value,
            sv.inputs.len()
        );
        if ui.selectable_label(current == s, text).clicked() {
            picked = Some(s);
        }
    }
    picked
}

/// 离指针最近且在命中半径内的控制点。
fn nearest(
    mapping: &InputMapping,
    p: egui::Pos2,
    sx: &impl Fn(f32) -> f32,
    sy: &impl Fn(f32) -> f32,
) -> Option<usize> {
    mapping
        .points
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| (i, (sx(x) - p.x).hypot(sy(y) - p.y)))
        .filter(|(_, d)| *d <= HIT_R)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// 取一个"好看"的坐标上限。
fn nice_ceil(v: f32) -> f32 {
    const NICE: [f32; 12] = [
        1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 64.0,
    ];
    NICE.iter().copied().find(|n| *n >= v).unwrap_or(v)
}

/// 在保持 x 升序的前提下插入控制点，返回插入后的下标。
///
/// libmypaint 的曲线要求 x 严格升序（`mypaint_mapping_calculate` 从左到右扫描），
/// 所以插入位置必须由二分确定，不能简单 push。
fn insert_point(points: &mut Vec<(f32, f32)>, x: f32, y: f32) -> usize {
    let at = points.partition_point(|(px, _)| *px <= x);
    points.insert(at, (x, y));
    at
}

/// 移动第 `i` 个控制点：x 被相邻控制点夹住（保证升序不破），y 夹在 `±ymax`。
/// 返回是否真的发生了变化。
fn move_point(
    points: &mut [(f32, f32)],
    i: usize,
    x: f32,
    y: f32,
    xmax: f32,
    ymax: f32,
) -> bool {
    if i >= points.len() {
        return false;
    }
    let lo = if i > 0 { points[i - 1].0 } else { 0.0 };
    let hi = if i + 1 < points.len() {
        points[i + 1].0
    } else {
        xmax
    };
    let next = (x.clamp(lo, hi), y.clamp(-ymax, ymax));
    if points[i] == next {
        return false;
    }
    points[i] = next;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_keeps_ascending_order() {
        let mut pts = vec![(0.0, 0.0), (1.0, 1.0)];
        let at = insert_point(&mut pts, 0.5, 2.0);
        assert_eq!(at, 1);
        assert_eq!(pts, vec![(0.0, 0.0), (0.5, 2.0), (1.0, 1.0)]);

        // 两端
        assert_eq!(insert_point(&mut pts, -1.0, 0.0), 0);
        assert_eq!(insert_point(&mut pts, 9.0, 0.0), pts.len() - 1);
        // 允许重复 x（官方 Posterizer 的阶梯曲线就是靠重复 x 构造的）
        let at = insert_point(&mut pts, 0.0, 5.0);
        assert!(pts.windows(2).all(|w| w[0].0 <= w[1].0), "升序被破坏: {pts:?}");
        assert_eq!(pts[at], (0.0, 5.0));
    }

    #[test]
    fn move_clamps_between_neighbors() {
        let mut pts = vec![(0.0, 0.0), (0.5, 0.0), (1.0, 0.0)];
        // 中间点往上限外拖 → x 被右邻居夹住，y 被 ymax 夹住
        assert!(move_point(&mut pts, 1, 2.0, 5.0, 1.0, 1.0));
        assert_eq!(pts[1], (1.0, 1.0));
        // 往回拖 → x 被左邻居夹住
        assert!(move_point(&mut pts, 1, -3.0, -5.0, 1.0, 1.0));
        assert_eq!(pts[1], (0.0, -1.0));
    }

    #[test]
    fn move_clamps_edges() {
        let mut pts = vec![(0.0, 0.0), (1.0, 0.0)];
        // 先做一次合法移动，避免"原地不动被当成失败"
        assert!(move_point(&mut pts, 0, 0.5, 0.5, 4.0, 2.0));
        assert_eq!(pts[0], (0.5, 0.5));
        // 首点：x 下界 0（不可能拖成负数）
        assert!(move_point(&mut pts, 0, -2.0, 0.5, 4.0, 2.0));
        assert_eq!(pts[0], (0.0, 0.5));
        // 末点：x 上界 = xmax，y 上界 = ymax
        assert!(move_point(&mut pts, 1, 9.0, 9.0, 4.0, 2.0));
        assert_eq!(pts[1], (4.0, 2.0));
        // 越界下标安全（不改动、不 panic）
        assert!(!move_point(&mut pts, 9, 1.0, 1.0, 4.0, 2.0));
    }

    #[test]
    fn move_reports_noop() {
        let mut pts = vec![(0.0, 0.0), (1.0, 1.0)];
        assert!(!move_point(&mut pts, 1, 1.0, 1.0, 1.0, 1.0));
    }

    /// 任意插入 / 拖动序列之后，x 必须始终非递减（libmypaint 的硬约束）。
    #[test]
    fn arbitrary_edits_never_break_order() {
        let mut pts: Vec<(f32, f32)> = vec![(0.0, 0.0)];
        let (xmax, ymax) = (4.0, 2.0);
        let mut seed = 12345u32;
        let mut rand = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / 16_777_216.0
        };
        for _ in 0..200 {
            if rand() < 0.4 {
                let x = rand() * xmax;
                insert_point(&mut pts, x, (rand() * 2.0 - 1.0) * ymax);
            } else if !pts.is_empty() {
                let i = (rand() * pts.len() as f32) as usize % pts.len();
                move_point(
                    &mut pts,
                    i,
                    rand() * xmax * 1.5 - xmax * 0.25,
                    (rand() * 3.0 - 1.5) * ymax,
                    xmax,
                    ymax,
                );
            }
            assert!(
                pts.windows(2).all(|w| w[0].0 <= w[1].0),
                "升序被破坏: {pts:?}"
            );
            assert!(pts.iter().all(|(_, y)| y.abs() <= ymax + 1e-6));
        }
    }
}
