//! 视图变换：世界像素坐标 ↔ 屏幕像素坐标（平移 + 以锚点缩放）。
//!
//! 约定：`screen_phys = world * zoom + offset_phys`，其中 `zoom` 是
//! **物理屏幕像素 / 世界像素**。UI 层（egui）的坐标是逻辑点，所以
//! [`View::world_to_screen`] / [`View::screen_to_world`] 在边界按
//! [`View::ppp`] 换算：`screen_pt = screen_phys / ppp`。
//!
//! 这与 MyPaint/GTK 在本机的实际语义一致：`scale=1.0` 时一个文档像素
//! 恰好占一个物理屏幕像素（150% Windows 缩放下 GTK 的 scale factor 仍是
//! 1.0）。**不能**把 egui 的 ppp 再乘进 zoom，否则 100% 会变成 150%。

/// 缩放阶梯 —— 逐项照搬 `gui/document.py:313-337` 的 `zoomlevel_values`。
///
/// 键盘缩放（`ZoomIn`/`ZoomOut`，`,` / `.`）不是连续乘系数，而是在这个固定
/// 阶梯上走一格（`document.py:1776-1816`）：先找当前 scale 在阶梯上的下标，
/// 不在阶梯上就按"插进排序后的位置"算，然后 ±1 并夹到两端。
pub const ZOOM_LEVELS: [f32; 20] = [
    // micro
    1.0 / 16.0,
    1.0 / 8.0,
    2.0 / 11.0,
    0.25,
    1.0 / 3.0,
    0.50,
    2.0 / 3.0,
    // normal
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    5.5,
    8.0,
    // macro
    11.0,
    16.0,
    23.0,
    32.0,
    45.0,
    64.0,
];

/// 缩放下界 —— `document.py:341`：`tdw.zoom_min = min(zoomlevel_values)`。
pub const ZOOM_MIN: f32 = 1.0 / 16.0;
/// 缩放上界 —— `document.py:342`：`tdw.zoom_max = max(zoomlevel_values)`。
///
/// 注意 `tileddrawwidget.py:711-712` 自己有一组默认值（1/5..5），
/// 是 `document.py` 在初始化时**覆盖**成阶梯端点的。
pub const ZOOM_MAX: f32 = 64.0;

/// 光标键步进平移的量 —— `gui/document.py:230`：`PAN_STEP = 0.2`
/// （"Stepwise panning amount: proportion of the canvas size"）。
pub const PAN_STEP: f32 = 0.2;

// ---- 滚轮规则（`gui/mode.py:433-585` 的 `ScrollableModeMixin`）----
//
// 源码的 `_PIXELS_PER_SMOOTH_SCROLL_UNIT = 25.0` 把 GTK 平滑滚动单位换成
// 屏幕像素；egui 的 `smooth_scroll_delta` 是"点"，鼠标滚轮一格约 50 点，
// 所以这里把 50 点当作源码的一个平滑单位（技术栈换算，测试固定）。

/// egui 平滑滚动点 / 源码平滑单位（一格滚轮 ≈ 50 点）。
pub const WHEEL_POINTS_PER_UNIT: f32 = 50.0;
/// `mode.py:442` 的 `_PIXELS_PER_SMOOTH_SCROLL_UNIT = 25.0`。
pub const WHEEL_PIXELS_PER_UNIT: f32 = 25.0;
/// 源码平滑缩放：`exp(dy / 100)`，其中 `dy` 已乘 25 → `exp(单位 * 0.25)`。
pub const WHEEL_ZOOM_EXP_PER_UNIT: f32 = 0.25;

/// 平滑滚动点 → 源码平滑单位。
pub fn wheel_units(delta: f32) -> f32 {
    delta / WHEEL_POINTS_PER_UNIT
}

/// 平滑缩放倍率（`mode.py:514-526`）。`delta_y > 0`（向上滚）放大。
pub fn wheel_zoom_factor(delta_y: f32) -> f32 {
    (wheel_units(delta_y) * WHEEL_ZOOM_EXP_PER_UNIT).exp()
}

/// 平滑平移的屏幕像素位移（`mode.py:564-568` + `tdw.scroll` 的取反）。
pub fn wheel_pan_delta(delta_x: f32, delta_y: f32) -> [f32; 2] {
    [
        -wheel_units(delta_x) * WHEEL_PIXELS_PER_UNIT,
        wheel_units(delta_y) * WHEEL_PIXELS_PER_UNIT,
    ]
}

/// Shift + 滚轮的纵向分量（平台适配）。
///
/// 源码只读 `delta_y`（`mode.py:503-508`），但 Windows 上按住 Shift 转滚轮
/// 常被驱动/winit 报成**水平**滚动：`delta_y = 0`、`delta_x ≠ 0`。此时
/// 默认的缩放分支会完全没反应。这里把水平分量折算回纵向：Windows 的
/// `WM_MOUSEHWHEEL` 正方向是"向右"，而 Shift+滚轮向上通常报负的 x，
/// 取反后与"向上 = 放大"一致。
///
/// 只在**缩放**分支用；平移分支照旧用 x 分量水平平移（源码 `:553-558`）。
pub fn shift_wheel_vertical(delta_x: f32, delta_y: f32) -> f32 {
    if delta_y == 0.0 && delta_x != 0.0 {
        -delta_x
    } else {
        delta_y
    }
}

/// Ctrl/Alt 反转缩放与平移（`mode.py:481-488`）。
///
/// `default_pan` 来自设备设置（源码 `ScrollAction`；本项目用偏好
/// `input.scroll_action`，默认 `zoom`）。返回 true 表示本次按平移处理。
pub fn wheel_action_is_pan(default_pan: bool, ctrl: bool, alt: bool) -> bool {
    default_pan ^ (ctrl || alt)
}

/// 无限画布的 2D 视图。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// 缩放倍率（1.0 = 1 世界像素对应 1 屏幕**物理**像素）。
    pub zoom: f32,
    /// 屏幕偏移（世界原点映射到的屏幕物理像素位置）—— 但对外暴露的是
    /// **逻辑点**；物理值 = `offset * ppp`。
    pub offset: [f32; 2],
    /// 视图旋转（弧度，正 = 顺时针；`tdw.rotation`）。
    pub rotation: f32,
    /// 视图镜像（`tdw.mirrored`，翻转屏幕 X 轴）。
    pub mirrored: bool,
    /// egui 逻辑点 / 物理像素。UI 边界换算用；不参与持久化。
    pub ppp: f32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: [0.0, 0.0],
            rotation: 0.0,
            mirrored: false,
            ppp: 1.0,
        }
    }
}

impl View {
    /// 更新 `points-per-pixel`；每次 UI 帧开始时由 app 从 `egui::Context` 同步。
    pub fn set_ppp(&mut self, ppp: f32) {
        if ppp.is_finite() && ppp > 0.0 {
            self.ppp = ppp;
        }
    }

    #[inline]
    pub fn world_to_screen(&self, wx: f32, wy: f32) -> [f32; 2] {
        let (s, c) = self.rotation.sin_cos();
        let x = wx * self.zoom / self.ppp;
        let y = wy * self.zoom / self.ppp;
        let mut rx = c * x - s * y;
        let ry = s * x + c * y;
        if self.mirrored {
            rx = -rx;
        }
        [self.offset[0] + rx, self.offset[1] + ry]
    }

    #[inline]
    pub fn screen_to_world(&self, sx: f32, sy: f32) -> [f32; 2] {
        let (s, c) = self.rotation.sin_cos();
        let mut dx = (sx - self.offset[0]) * self.ppp;
        let dy = (sy - self.offset[1]) * self.ppp;
        if self.mirrored {
            dx = -dx;
        }
        [
            (c * dx + s * dy) / self.zoom,
            (-s * dx + c * dy) / self.zoom,
        ]
    }

    /// 物理屏幕像素输入版（供 `visible_tile_range` / GPU 侧使用）。
    #[inline]
    pub fn screen_to_world_phys(&self, sx: f32, sy: f32) -> [f32; 2] {
        self.screen_to_world(sx / self.ppp, sy / self.ppp)
    }

    /// 平移（屏幕像素增量）。
    #[inline]
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.offset[0] += dx;
        self.offset[1] += dy;
    }

    /// 以屏幕锚点为中心旋转（`TiledDrawWidget.rotate` 的 `_fixed_center`）。
    pub fn rotate_at(&mut self, step: f32, ax: f32, ay: f32) {
        let before = self.screen_to_world(ax, ay);
        self.rotation += step;
        let after = self.world_to_screen(before[0], before[1]);
        self.offset[0] += ax - after[0];
        self.offset[1] += ay - after[1];
    }

    /// 交互式旋转一步，镜像时取反（`tileddrawwidget.py:422-427` 的 `rotate`）。
    ///
    /// 源码镜像后屏幕 X 轴翻转，若步长不取反，拖拽/快捷键的旋转方向会与指针
    /// 相反。`ResetRotation` 不走这里：源码用 `set_rotation(0)`（`:1959-1960`），
    /// 对 0 取不取反都一样。
    pub fn rotate_step_at(&mut self, step: f32, ax: f32, ay: f32) {
        let step = if self.mirrored { -step } else { step };
        self.rotate_at(step, ax, ay);
    }

    /// 翻转镜像，并保持屏幕锚点处的世界坐标不动（`mirror()`）。
    pub fn mirror_at(&mut self, ax: f32, ay: f32) {
        let before = self.screen_to_world(ax, ay);
        self.mirrored = !self.mirrored;
        let after = self.world_to_screen(before[0], before[1]);
        self.offset[0] += ax - after[0];
        self.offset[1] += ay - after[1];
    }

    /// 以屏幕锚点 `(ax, ay)` 为中心缩放（缩放前后锚点处的世界坐标不动）。
    ///
    /// 钳位范围 = 阶梯端点（`ZOOM_MIN..ZOOM_MAX`），对应
    /// `tileddrawwidget._fixed_center` 里那句
    /// `scale = clamp(scale, zoom_min, zoom_max)`（`:400-402`）。
    pub fn zoom_at(&mut self, ax: f32, ay: f32, factor: f32) {
        let target = (self.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        self.set_zoom_at(target, ax, ay);
    }

    /// 把缩放设成**精确值**，并保持屏幕锚点 `(ax, ay)` 处的世界坐标不动。
    ///
    /// 对应 `TiledDrawWidget.set_zoom(zoom, center)` + `_fixed_center`：
    /// 改完 scale 之后再把中心点挪回原位（`tileddrawwidget.py:403-405`）。
    pub fn set_zoom_at(&mut self, zoom: f32, ax: f32, ay: f32) {
        let before = self.screen_to_world(ax, ay);
        self.zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let after_screen = self.world_to_screen(before[0], before[1]);
        self.offset[0] += ax - after_screen[0];
        self.offset[1] += ay - after_screen[1];
    }

    /// 在 [`ZOOM_LEVELS`] 阶梯上走一格，返回新的缩放值（不改自身状态）。
    ///
    /// 照搬 `Document.zoom()`（`gui/document.py:1794-1814`）：
    /// ```python
    /// try:
    ///     zoom_index = self.zoomlevel_values.index(self.tdw.scale)
    /// except ValueError:                 # 不在阶梯上（滚轮缩放过）
    ///     zoom_levels = sorted(self.zoomlevel_values + [self.tdw.scale])
    ///     zoom_index = zoom_levels.index(self.tdw.scale)
    /// zoom_index += 1 if 放大 else -1
    /// clamp(zoom_index, 0, len - 1)
    /// ```
    pub fn stepped_zoom(&self, zoom_in: bool) -> f32 {
        let idx = match ZOOM_LEVELS.iter().position(|z| *z == self.zoom) {
            Some(i) => i as i32,
            None => {
                // 非阶梯值：先插进排序后的位置（照 Python 的 except 分支）
                let mut levels = ZOOM_LEVELS.to_vec();
                levels.push(self.zoom);
                levels.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                levels.iter().position(|z| *z == self.zoom).unwrap_or(0) as i32
            }
        };
        let idx = if zoom_in { idx + 1 } else { idx - 1 };
        let idx = idx.clamp(0, ZOOM_LEVELS.len() as i32 - 1) as usize;
        ZOOM_LEVELS[idx]
    }

    /// `FitView` 的视图变换（`gui/document.py:1991-2031`）。
    ///
    /// 源码先暂存 rotation/mirror，按未旋转的 bbox 计算旋转后的外接尺寸，
    /// 取 `min((w1-20)/x, (h1-20)/y)`，然后 recenter 并恢复 rotation/mirror。
    /// 这里直接把 bbox 中心映射到视口中心，旋转/镜像下的结果与源码一致。
    ///
    /// 空文档走 `reset_view(True, True, True)` 分支：零旋转、零镜像、
    /// 默认缩放，并把世界原点放到视口中心。
    pub fn fit_to_bbox(
        &self,
        bbox: (i32, i32, i32, i32),
        viewport: [f32; 2],
        default_zoom: f32,
    ) -> View {
        let (vw, vh) = (viewport[0].max(0.0), viewport[1].max(0.0));
        let (bx, by, bw, bh) = bbox;
        if bw <= 0 || bh <= 0 {
            return View {
                zoom: default_zoom.clamp(ZOOM_MIN, ZOOM_MAX),
                offset: [vw / 2.0, vh / 2.0],
                rotation: 0.0,
                mirrored: false,
                ppp: self.ppp,
            };
        }
        let w = bw as f32;
        let h = bh as f32;
        let (sin, cos) = self.rotation.sin_cos();
        // 旋转后 bbox 的轴向尺寸（document.py:2009-2023）
        let x_extent = (w * cos - h * sin).abs().max((w * cos + h * sin).abs());
        let y_extent = (w * sin + h * cos).abs().max((w * sin - h * cos).abs());
        // 20px 留白是屏幕物理像素；viewport 是逻辑点，先换到物理域。
        let vw_phys = vw * self.ppp;
        let vh_phys = vh * self.ppp;
        let zoom = ((vw_phys - 20.0) / x_extent.max(f32::EPSILON))
            .min((vh_phys - 20.0) / y_extent.max(f32::EPSILON))
            .clamp(ZOOM_MIN, ZOOM_MAX);
        // bbox 中心 → 视口中心；先算物理屏幕偏移，再换回逻辑点。
        let cx = bx as f32 + w / 2.0;
        let cy = by as f32 + h / 2.0;
        let mut rx = cos * (cx * zoom) - sin * (cy * zoom);
        let ry = sin * (cx * zoom) + cos * (cy * zoom);
        if self.mirrored {
            rx = -rx;
        }
        View {
            zoom,
            offset: [vw / 2.0 - rx / self.ppp, vh / 2.0 - ry / self.ppp],
            rotation: self.rotation,
            mirrored: self.mirrored,
            ppp: self.ppp,
        }
    }

    /// 屏幕矩形（物理像素）覆盖的世界 tile 坐标范围（两端均含）。
    /// 返回 `(tx_min, ty_min, tx_max, ty_max)`。
    ///
    /// 世界区间视为 `[x0, x1)` 半开：右/下边缘恰好落在 tile 边界时不多取一块。
    pub fn visible_tile_range(&self, w: f32, h: f32, tile_size: i32) -> (i32, i32, i32, i32) {
        const EPS: f32 = 1e-4;
        let corners = [
            self.screen_to_world_phys(0.0, 0.0),
            self.screen_to_world_phys(w, 0.0),
            self.screen_to_world_phys(0.0, h),
            self.screen_to_world_phys(w, h),
        ];
        let xmin = corners.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let xmax = corners
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let ymin = corners.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let ymax = corners
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let tile_of = |v: f32| (v.floor() as i32).div_euclid(tile_size);
        (
            tile_of(xmin),
            tile_of(ymin),
            tile_of(xmax - EPS),
            tile_of(ymax - EPS),
        )
    }

    /// 世界像素 → 裁剪空间的 4×4 矩阵（列主序，供 WGSL uniform）。
    /// `viewport` 为渲染目标**物理**像素尺寸；`offset` 是逻辑点，矩阵里
    /// 先乘 `ppp` 换回物理域。
    pub fn clip_matrix(&self, viewport_w: f32, viewport_h: f32) -> [[f32; 4]; 4] {
        let (s, c) = self.rotation.sin_cos();
        // screen_phys = offset_phys + R(rotation) * (zoom * world)，
        // mirrored 时再翻转 X。此处 zoom 已是物理像素 / 世界像素。
        let mut ax = self.zoom * c;
        let mut bx = -self.zoom * s;
        let ay = self.zoom * s;
        let by = self.zoom * c;
        if self.mirrored {
            ax = -ax;
            bx = -bx;
        }
        let ox = self.offset[0] * self.ppp;
        let oy = self.offset[1] * self.ppp;
        let tx = 2.0 * ox / viewport_w - 1.0;
        let ty = 1.0 - 2.0 * oy / viewport_h;
        [
            [2.0 * ax / viewport_w, -2.0 * ay / viewport_h, 0.0, 0.0],
            [2.0 * bx / viewport_w, -2.0 * by / viewport_h, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [tx, ty, 0.0, 1.0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let v = View {
            zoom: 2.5,
            offset: [123.0, -45.0],
            ..View::default()
        };
        let w = v.screen_to_world(400.0, 300.0);
        let s = v.world_to_screen(w[0], w[1]);
        assert!((s[0] - 400.0).abs() < 1e-4 && (s[1] - 300.0).abs() < 1e-4);
    }

    #[test]
    fn zoom_at_keeps_anchor_fixed() {
        let mut v = View::default();
        let anchor_world = v.screen_to_world(200.0, 150.0);
        v.zoom_at(200.0, 150.0, 1.7);
        let after = v.world_to_screen(anchor_world[0], anchor_world[1]);
        assert!((after[0] - 200.0).abs() < 1e-3 && (after[1] - 150.0).abs() < 1e-3);
    }

    #[test]
    fn fit_to_bbox_centers_and_preserves_rotation_mirror() {
        // 未旋转：100×50 的 bbox 在 200×200 视口里受宽度限制 → zoom=180/100=1.8
        let v = View::default();
        let fit = v.fit_to_bbox((0, 0, 100, 50), [200.0, 200.0], 1.0);
        assert!((fit.zoom - 1.8).abs() < 1e-5, "zoom={}", fit.zoom);
        let center = fit.world_to_screen(50.0, 25.0);
        assert!((center[0] - 100.0).abs() < 1e-4 && (center[1] - 100.0).abs() < 1e-4);

        // 旋转 90°：轴向尺寸互换，高度变成限制项 → 仍是 1.8，中心仍在视口中心
        let v = View {
            rotation: std::f32::consts::FRAC_PI_2,
            ..View::default()
        };
        let fit = v.fit_to_bbox((0, 0, 100, 50), [200.0, 200.0], 1.0);
        assert!((fit.zoom - 1.8).abs() < 1e-5, "zoom={}", fit.zoom);
        assert!((fit.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let center = fit.world_to_screen(50.0, 25.0);
        assert!((center[0] - 100.0).abs() < 1e-4 && (center[1] - 100.0).abs() < 1e-4);

        // 镜像同理：只翻 X，不改变 bbox 中心落点
        let v = View {
            mirrored: true,
            ..View::default()
        };
        let fit = v.fit_to_bbox((10, 20, 100, 50), [300.0, 200.0], 1.0);
        assert!(fit.mirrored);
        let center = fit.world_to_screen(60.0, 45.0);
        assert!((center[0] - 150.0).abs() < 1e-4 && (center[1] - 100.0).abs() < 1e-4);
    }

    #[test]
    fn fit_to_empty_bbox_resets_view_like_source() {
        let v = View {
            zoom: 3.0,
            offset: [7.0, 9.0],
            rotation: 0.4,
            mirrored: true,
            ppp: 1.0,
        };
        let fit = v.fit_to_bbox((0, 0, 0, 0), [200.0, 100.0], 1.25);
        assert_eq!(fit.zoom, 1.25);
        assert_eq!(fit.rotation, 0.0);
        assert!(!fit.mirrored);
        assert_eq!(fit.offset, [100.0, 50.0]);
    }

    #[test]
    fn clip_matrix_maps_corners() {
        // 视口 800×600，zoom=1，offset=(0,0)：世界 (0,0)→(-1,1)，(800,600)→(1,-1)
        let v = View::default();
        let m = v.clip_matrix(800.0, 600.0);
        let mul = |x: f32, y: f32| {
            (
                m[0][0] * x + m[1][0] * y + m[3][0],
                m[0][1] * x + m[1][1] * y + m[3][1],
            )
        };
        let (x0, y0) = mul(0.0, 0.0);
        assert!((x0 + 1.0).abs() < 1e-6 && (y0 - 1.0).abs() < 1e-6);
        let (x1, y1) = mul(800.0, 600.0);
        assert!((x1 - 1.0).abs() < 1e-6 && (y1 + 1.0).abs() < 1e-6);
    }

    #[test]
    fn visible_range_covers_screen_edges() {
        let v = View {
            zoom: 1.0,
            offset: [0.0, 0.0],
            ..View::default()
        };
        // 128×128 视口恰好覆盖 tile (0,0) 与 (1,1)
        let r = v.visible_tile_range(128.0, 128.0, 64);
        assert_eq!(r, (0, 0, 1, 1));
    }

    #[test]
    fn zoom_100_is_one_world_px_per_physical_px_at_150pct() {
        // 本机 GTK/MyPaint 在 150% Windows 缩放下 scale factor 仍是 1.0：
        // 100% 缩放 = 1 文档像素占 1 **物理**像素，而不是 1 逻辑点。
        let v = View {
            zoom: 1.0,
            ppp: 1.5,
            ..View::default()
        };
        let w0 = v.world_to_screen(0.0, 0.0);
        let w256 = v.world_to_screen(256.0, 0.0);
        let phys_span = (w256[0] - w0[0]) * v.ppp;
        assert!((phys_span - 256.0).abs() < 1e-4, "phys_span={phys_span}");
        // 反向：物理 384px = 世界 384px（不是 256）。
        let back = v.screen_to_world_phys(384.0, 0.0);
        assert!((back[0] - 384.0).abs() < 1e-4, "world={}", back[0]);
        // 可见 tile 范围也按物理像素解释：128px 物理覆盖两块 64px tile。
        let r = v.visible_tile_range(128.0, 128.0, 64);
        assert_eq!(r, (0, 0, 1, 1));
        // 裁剪矩阵的视口是物理像素：世界 256 应落在 256/1500 的 NDC。
        let m = v.clip_matrix(1500.0, 900.0);
        let ndc_x = m[0][0] * 256.0 + m[3][0];
        assert!((ndc_x - (2.0 * 256.0 / 1500.0 - 1.0)).abs() < 1e-5);
    }

    #[test]
    fn zoom_ladder_matches_document_py() {
        // 逐项照搬 document.py:313-337
        assert_eq!(ZOOM_LEVELS.len(), 20);
        assert_eq!(ZOOM_LEVELS[0], 1.0 / 16.0);
        assert_eq!(ZOOM_LEVELS[7], 1.0);
        assert_eq!(ZOOM_LEVELS[19], 64.0);
        assert_eq!(ZOOM_LEVELS.windows(2).filter(|w| w[1] <= w[0]).count(), 0);
        // 上下界 = 阶梯端点（document.py:341-342 覆盖了 tdw 的默认值）
        assert_eq!(ZOOM_MIN, ZOOM_LEVELS[0]);
        assert_eq!(ZOOM_MAX, ZOOM_LEVELS[19]);
    }

    #[test]
    fn stepped_zoom_walks_the_ladder_and_saturates() {
        let at = |z: f32| View {
            zoom: z,
            offset: [0.0, 0.0],
            ..View::default()
        };
        // 1.0 在阶梯上（下标 7）：放大 → 1.5，缩小 → 2/3
        assert_eq!(at(1.0).stepped_zoom(true), 1.5);
        assert_eq!(at(1.0).stepped_zoom(false), 2.0 / 3.0);
        // 两端饱和（Python 的 clamp 到 [0, len-1]）
        assert_eq!(at(ZOOM_MAX).stepped_zoom(true), ZOOM_MAX);
        assert_eq!(at(ZOOM_MIN).stepped_zoom(false), ZOOM_MIN);
        // 不在阶梯上（滚轮缩放到 0.7）：插进排序后的位置 —— 排序列表里 0.7
        // 落在 2/3 与 1.0 之间（下标 7），所以放大走下标 8 = 1.5、
        // 缩小走下标 6 = 2/3。注意**放大是从"插进去的位置"+1**，
        // 不是"大于 0.7 的最小阶梯值"（两者在这里恰好都指向下一格，
        // 但 0.7 紧贴 2/3 时就用得上这条区别了）。
        assert_eq!(at(0.7).stepped_zoom(true), 1.5);
        assert_eq!(at(0.7).stepped_zoom(false), 2.0 / 3.0);
        // 比阶梯最小值还小 → 缩小仍是下界，放大到 1/8
        assert_eq!(at(0.01).stepped_zoom(false), ZOOM_LEVELS[0]);
        assert_eq!(at(0.01).stepped_zoom(true), ZOOM_LEVELS[1]);
    }

    #[test]
    fn set_zoom_at_keeps_anchor_and_clamps() {
        let mut v = View::default();
        let anchor_world = v.screen_to_world(300.0, 200.0);
        v.set_zoom_at(4.0, 300.0, 200.0);
        assert_eq!(v.zoom, 4.0);
        let after = v.world_to_screen(anchor_world[0], anchor_world[1]);
        assert!((after[0] - 300.0).abs() < 1e-3 && (after[1] - 200.0).abs() < 1e-3);
        // 超出阶梯端点 → 钳到端点（不是 0.05/32 那套旧值）
        v.set_zoom_at(1000.0, 300.0, 200.0);
        assert_eq!(v.zoom, ZOOM_MAX);
        v.set_zoom_at(0.0001, 300.0, 200.0);
        assert_eq!(v.zoom, ZOOM_MIN);
    }

    #[test]
    fn rotated_mirrored_roundtrip_and_anchor() {
        let mut v = View::default();
        let anchor_world = v.screen_to_world(400.0, 300.0);
        v.rotate_at(std::f32::consts::TAU / 16.0, 400.0, 300.0);
        v.mirror_at(400.0, 300.0);
        let w = v.screen_to_world(123.0, 77.0);
        let s = v.world_to_screen(w[0], w[1]);
        assert!((s[0] - 123.0).abs() < 1e-3 && (s[1] - 77.0).abs() < 1e-3);
        let after = v.world_to_screen(anchor_world[0], anchor_world[1]);
        assert!((after[0] - 400.0).abs() < 1e-3);
        assert!((after[1] - 300.0).abs() < 1e-3);
    }

    /// `tdw.rotate` 在镜像时把步长取反（`tileddrawwidget.py:422-427`）。
    #[test]
    fn rotate_step_negates_when_mirrored_and_keeps_anchor() {
        let mut v = View {
            rotation: 0.4,
            offset: [10.0, 20.0],
            mirrored: true,
            ..View::default()
        };
        let anchor_world = v.screen_to_world(150.0, 90.0);
        v.rotate_step_at(0.3, 150.0, 90.0);
        assert!((v.rotation - 0.1).abs() < 1e-6, "rotation={}", v.rotation);
        let after = v.world_to_screen(anchor_world[0], anchor_world[1]);
        assert!((after[0] - 150.0).abs() < 1e-3 && (after[1] - 90.0).abs() < 1e-3);

        let mut v = View {
            rotation: 0.4,
            ..View::default()
        };
        v.rotate_step_at(0.3, 150.0, 90.0);
        assert!((v.rotation - 0.7).abs() < 1e-6, "未镜像保持原方向");
    }

    #[test]
    fn rotated_clip_matrix_maps_a_quarter_turn() {
        let v = View {
            zoom: 1.0,
            offset: [0.0, 0.0],
            rotation: std::f32::consts::FRAC_PI_2,
            mirrored: false,
            ppp: 1.0,
        };
        let m = v.clip_matrix(800.0, 600.0);
        let mul = |x: f32, y: f32| {
            (
                m[0][0] * x + m[1][0] * y + m[3][0],
                m[0][1] * x + m[1][1] * y + m[3][1],
            )
        };
        // 世界 (100,0) 顺时针转 90° → 屏幕 (0,100) → 裁剪 (-1, 1-200/600)
        let (x, y) = mul(100.0, 0.0);
        assert!((x + 1.0).abs() < 1e-5);
        assert!((y - (1.0 - 200.0 / 600.0)).abs() < 1e-5);
    }

    #[test]
    fn wheel_zoom_matches_the_source_smooth_formula() {
        // 一格（50 点）向上 → exp(0.25) ≈ 1.284；向下 → exp(-0.25) ≈ 0.779
        assert!((wheel_zoom_factor(50.0) - 0.25f32.exp()).abs() < 1e-5);
        assert!((wheel_zoom_factor(-50.0) - (-0.25f32).exp()).abs() < 1e-5);
        assert_eq!(wheel_zoom_factor(0.0), 1.0);
    }

    #[test]
    fn wheel_pan_uses_25_pixels_per_unit_and_inverts() {
        // 向上滚（+y）→ 内容下移（+y）；向右滚（+x）→ 内容左移（-x）
        let [dx, dy] = wheel_pan_delta(50.0, 50.0);
        assert!((dx + WHEEL_PIXELS_PER_UNIT).abs() < 1e-5);
        assert!((dy - WHEEL_PIXELS_PER_UNIT).abs() < 1e-5);
        let [dx, dy] = wheel_pan_delta(-100.0, -50.0);
        assert!((dx - 2.0 * WHEEL_PIXELS_PER_UNIT).abs() < 1e-5);
        assert!((dy + WHEEL_PIXELS_PER_UNIT).abs() < 1e-5);
    }

    #[test]
    fn shift_wheel_horizontal_falls_back_to_vertical_for_zoom() {
        // 正常纵向滚轮原样保留
        assert_eq!(shift_wheel_vertical(0.0, 50.0), 50.0);
        assert_eq!(shift_wheel_vertical(30.0, -50.0), -50.0);
        // Windows 的 Shift+滚轮只剩水平分量：向上（负 x）→ 放大（正 y）
        assert_eq!(shift_wheel_vertical(-50.0, 0.0), 50.0);
        assert_eq!(shift_wheel_vertical(50.0, 0.0), -50.0);
        // 两个分量都有时以纵向为准，不做二次叠加
        assert_eq!(shift_wheel_vertical(50.0, 20.0), 20.0);
        assert_eq!(shift_wheel_vertical(0.0, 0.0), 0.0);
        // 折算后的倍率方向与普通滚轮一致
        assert!(wheel_zoom_factor(shift_wheel_vertical(-50.0, 0.0)) > 1.0);
    }

    #[test]
    fn ctrl_or_alt_flips_zoom_and_pan() {
        // 默认 zoom：不按修饰键 = 缩放；Ctrl 或 Alt = 平移
        assert!(!wheel_action_is_pan(false, false, false));
        assert!(wheel_action_is_pan(false, true, false));
        assert!(wheel_action_is_pan(false, false, true));
        // 默认 pan（设备设置）：反过来
        assert!(wheel_action_is_pan(true, false, false));
        assert!(!wheel_action_is_pan(true, true, false));
    }

    #[test]
    fn rotated_visible_range_covers_the_whole_screen() {
        let v = View {
            rotation: std::f32::consts::FRAC_PI_4,
            ..View::default()
        };
        let (x0, y0, x1, y1) = v.visible_tile_range(128.0, 128.0, 64);
        assert!(x0 <= 0 && y0 <= 0 && x1 >= 1 && y1 >= 1);
    }
}
