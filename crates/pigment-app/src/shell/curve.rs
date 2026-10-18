//! 压力曲线编辑器 —— `gui/curve.py` 的 `CurveWidget`（本移植的 egui 版）。
//!
//! 源码的曲线控件是一个通用 `Gtk.DrawingArea`：按住点拖动、点空白插入、
//! 把点拖出边界外删除；首尾点的 x 固定为 0/1，中间点夹在相邻点之间。
//! 偏好窗口只关掉磁吸（`curve.magnetic = False`，`preferenceswindow.py:80`），
//! 其余行为照搬。点列直接存进 `input.global_pressure_mapping`
//! （`preferenceswindow.py:276-279`），消费方是引擎的
//! `FreehandConfig::map_pressure`。
//!
//! 本文件只做**模型**（可单测）+ 一个 `curve_editor` 绘制/交互函数；
//! 磁吸、`npoints` 固定点数、`ylockgroups` 这几个通用能力当前用不到，
//! 未实现（记账：压力曲线 `magnetic=False`、点数不固定、无 y 锁）。

use eframe::egui;

/// 点命中/插入半径（源码用平方距离 `0.003` ≈ 0.0548）。
const HIT_DIST_SQ: f32 = 0.003;
/// 拖出边界的 x 余量（`curve.py:216` 的 `margin = 0.02`）。
const DRAG_MARGIN: f32 = 0.02;
/// y 的合法拖动范围（`curve.py:218`）。
const DRAG_Y_MIN: f32 = -0.1;
const DRAG_Y_MAX: f32 = 1.1;
/// 最大点数（`curve.py:81` 的 `maxpoints = 64`）。
const MAX_POINTS: usize = 64;
/// 绘制内缩（`curve.py:22` 的 `RADIUS = 2`）。
const RADIUS: f32 = 2.0;

/// 一次拖动更新的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// 点被更新。
    Moved,
    /// 点被拖出边界，应该删除（`curve.py:219-223` 的 `points[i] = None`）。
    Remove,
    /// 没有变化。
    Ignored,
}

/// 把偏好里的 `[[x,y], ...]` 读成点列；缺失/非法返回源码默认的
/// `[(0.0, 1.0), (1.0, 0.0)]`（`userconfig.py:64`）。
pub fn points_from_json(value: Option<&serde_json::Value>) -> Vec<(f32, f32)> {
    let default = vec![(0.0, 1.0), (1.0, 0.0)];
    let Some(arr) = value.and_then(|v| v.as_array()) else {
        return default;
    };
    let mut out: Vec<(f32, f32)> = arr
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            let x = p.first()?.as_f64()? as f32;
            let y = p.get(1)?.as_f64()? as f32;
            Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
        })
        .collect();
    if out.len() < 2 {
        return default;
    }
    // 源码假定点按 x 升序（插入算法依赖它）；偏好里乱序时保守排序。
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 点列 → 偏好 JSON（每项 `[x, y]`）。
pub fn points_to_json(points: &[(f32, f32)]) -> serde_json::Value {
    serde_json::Value::Array(
        points
            .iter()
            .map(|(x, y)| {
                serde_json::Value::Array(vec![
                    serde_json::json!(*x as f64),
                    serde_json::json!(*y as f64),
                ])
            })
            .collect(),
    )
}

/// 找最近点：返回 `(平方距离, 下标)`（`curve.py:172-175`）。
pub fn nearest(points: &[(f32, f32)], x: f32, y: f32) -> Option<(f32, usize)> {
    points
        .iter()
        .enumerate()
        .map(|(i, (px, py))| {
            let d = (x - px) * (x - px) + (y - py) * (y - py);
            (d, i)
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

/// 点击空白处插入一个点 —— `curve.py:176-189`。
///
/// 返回新点下标；点已满、或点太靠近现有点、或 x 不在首尾之间时不插入。
/// 调用方应先把 `points` 排序。
pub fn insert_point(points: &mut Vec<(f32, f32)>, x: f32, y: f32) -> Option<usize> {
    if points.len() >= MAX_POINTS || points.len() < 2 {
        return None;
    }
    if let Some((d, _)) = nearest(points, x, y) {
        if d <= HIT_DIST_SQ {
            return None;
        }
    }
    let pos = points.iter().rposition(|(px, _)| *px < x)? + 1;
    if pos >= points.len() {
        return None;
    }
    points.insert(pos, (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    Some(pos)
}

/// 拖动第 `i` 个点 —— `curve.py:193-230` 的 `motion_notify_cb`。
///
/// 首点 x 固定 0、尾点 x 固定 1；中间点夹在相邻点之间；x/y 拖出边界
/// （x 超出相邻点 ±0.02 或 y 超出 [-0.1, 1.1]）时返回 [`DragOutcome::Remove`]。
pub fn drag_point(points: &mut Vec<(f32, f32)>, i: usize, x: f32, y: f32) -> DragOutcome {
    if i >= points.len() {
        return DragOutcome::Ignored;
    }
    let last = points.len() - 1;
    let (left, right) = if i == last {
        (1.0, 1.0)
    } else if i == 0 {
        (0.0, 0.0)
    } else {
        (points[i - 1].0, points[i + 1].0)
    };
    if i != 0 && i != last {
        let inside_x = left - DRAG_MARGIN < x && x < right + DRAG_MARGIN;
        let inside_y = (DRAG_Y_MIN..=DRAG_Y_MAX).contains(&y);
        if !(inside_x && inside_y) {
            points.remove(i);
            return DragOutcome::Remove;
        }
    }
    let x = if i == 0 {
        0.0
    } else if i == last {
        1.0
    } else {
        x.clamp(left, right)
    };
    points[i] = (x, y.clamp(0.0, 1.0));
    DragOutcome::Moved
}

/// 绘制并处理压力曲线；有改动时返回 true。
///
/// `drag` 由调用方持有（egui 立即模式没有控件对象），记录正在拖的点。
pub fn curve_editor(
    ui: &mut egui::Ui,
    points: &mut Vec<(f32, f32)>,
    drag: &mut Option<usize>,
) -> bool {
    let size = egui::vec2(ui.available_width().min(360.0).max(160.0), 215.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let area = egui::Rect::from_min_max(
        rect.min + egui::vec2(RADIUS, RADIUS),
        rect.max - egui::vec2(RADIUS, RADIUS),
    );
    let to_screen = |(x, y): (f32, f32)| {
        egui::pos2(
            area.left() + x * area.width(),
            area.top() + y * area.height(),
        )
    };
    let from_screen = |p: egui::Pos2| {
        (
            ((p.x - area.left()) / area.width()).clamp(0.0, 1.0),
            ((p.y - area.top()) / area.height()).clamp(0.0, 1.0),
        )
    };

    // 网格（10×10）
    let grid = ui.visuals().weak_text_color().gamma_multiply(0.25);
    for i in 0..=10 {
        let t = i as f32 / 10.0;
        let x = area.left() + t * area.width();
        let y = area.top() + t * area.height();
        painter.line_segment(
            [egui::pos2(x, area.top()), egui::pos2(x, area.bottom())],
            egui::Stroke::new(0.5, grid),
        );
        painter.line_segment(
            [egui::pos2(area.left(), y), egui::pos2(area.right(), y)],
            egui::Stroke::new(0.5, grid),
        );
    }

    let mut changed = false;
    if resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let (x, y) = from_screen(pos);
            if let Some((d, i)) = nearest(points, x, y) {
                if d <= HIT_DIST_SQ {
                    *drag = Some(i);
                } else if let Some(i) = insert_point(points, x, y) {
                    *drag = Some(i);
                    changed = true;
                }
            }
        }
    }
    if let Some(i) = *drag {
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (x, y) = from_screen(pos);
                match drag_point(points, i, x, y) {
                    DragOutcome::Moved => changed = true,
                    DragOutcome::Remove => {
                        *drag = None;
                        changed = true;
                    }
                    DragOutcome::Ignored => {}
                }
            }
        }
        if resp.drag_stopped() {
            *drag = None;
        }
    }

    // 折线
    if points.len() >= 2 {
        let line: Vec<egui::Pos2> = points.iter().map(|p| to_screen(*p)).collect();
        painter.add(egui::Shape::line(
            line,
            egui::Stroke::new(1.5, ui.visuals().selection.bg_fill),
        ));
    }
    // 点
    for (i, p) in points.iter().enumerate() {
        let c = to_screen(*p);
        let active = *drag == Some(i);
        painter.circle_filled(
            c,
            if active { 4.0 } else { 3.0 },
            if active {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            },
        );
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_points() -> Vec<(f32, f32)> {
        vec![(0.0, 1.0), (1.0, 0.0)]
    }

    #[test]
    fn json_roundtrip_and_defaults() {
        let p = points_from_json(None);
        assert_eq!(p, default_points());
        let j = points_to_json(&p);
        assert_eq!(points_from_json(Some(&j)), p);
        // 乱序输入按 x 排序；少于两点回落默认
        let j = serde_json::json!([[1.0, 0.0], [0.0, 1.0], [0.5, 0.25]]);
        let p = points_from_json(Some(&j));
        assert_eq!(p, vec![(0.0, 1.0), (0.5, 0.25), (1.0, 0.0)]);
        assert_eq!(points_from_json(Some(&serde_json::json!([[0.2, 0.2]]))), default_points());
    }

    #[test]
    fn clicking_between_points_inserts_in_x_order() {
        let mut p = default_points();
        let i = insert_point(&mut p, 0.5, 0.4).expect("中间应能插点");
        assert_eq!(i, 1);
        assert_eq!(p, vec![(0.0, 1.0), (0.5, 0.4), (1.0, 0.0)]);
        // 太靠近现有点 → 不插
        assert!(insert_point(&mut p, 0.5, 0.4).is_none());
        // 超出尾点 → 不插
        assert!(insert_point(&mut p, 1.1, 0.2).is_none());
    }

    #[test]
    fn first_and_last_points_keep_their_x() {
        let mut p = vec![(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)];
        assert_eq!(drag_point(&mut p, 0, 0.7, 0.2), DragOutcome::Moved);
        assert_eq!(p[0].0, 0.0);
        assert_eq!(drag_point(&mut p, 2, 0.1, 0.8), DragOutcome::Moved);
        assert_eq!(p[2].0, 1.0);
        // 中间点夹在相邻 x 之间（0.9 仍在 [0, 1] 内）
        assert_eq!(drag_point(&mut p, 1, 0.9, 0.3), DragOutcome::Moved);
        assert_eq!(p[1], (0.9, 0.3));
    }

    #[test]
    fn dragging_out_removes_the_point() {
        let mut p = vec![(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)];
        assert_eq!(drag_point(&mut p, 1, 1.1, 0.5), DragOutcome::Remove);
        assert_eq!(p, vec![(0.0, 1.0), (1.0, 0.0)]);
        // y 拖出 [-0.1,1.1] 也删除
        let mut p = vec![(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)];
        assert_eq!(drag_point(&mut p, 1, 0.5, 1.3), DragOutcome::Remove);
    }

    #[test]
    fn drag_clamps_y_into_unit_range() {
        let mut p = vec![(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)];
        assert_eq!(drag_point(&mut p, 1, 0.5, -0.05), DragOutcome::Moved);
        assert_eq!(p[1].1, 0.0);
    }

    #[test]
    fn curve_points_reach_freehand_config_through_the_pref() {
        use pigment_engine::mode::freehand::FreehandConfig;
        let path = std::env::temp_dir().join(format!(
            "pigment-curve-pref-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let mut prefs = pigment_engine::prefs::Prefs::load(&path);
        let pts = vec![(0.0, 0.0), (0.5, 0.3), (1.0, 1.0)];
        prefs.set("input.global_pressure_mapping", points_to_json(&pts));
        let cfg = FreehandConfig::from_prefs(&prefs);
        assert_eq!(cfg.pressure_mapping, pts);
        let _ = std::fs::remove_file(&path);
    }
}
