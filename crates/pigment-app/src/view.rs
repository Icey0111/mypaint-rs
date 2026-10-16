//! 视图变换：世界像素坐标 ↔ 屏幕像素坐标（平移 + 以锚点缩放）。
//!
//! 约定：`screen = world * zoom + offset`（offset/zoom 均为屏幕物理像素域）。

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

/// 无限画布的 2D 视图。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// 缩放倍率（1.0 = 1 世界像素对应 1 屏幕物理像素）。
    pub zoom: f32,
    /// 屏幕偏移（世界原点映射到的屏幕物理像素位置）。
    pub offset: [f32; 2],
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: [0.0, 0.0],
        }
    }
}

impl View {
    #[inline]
    pub fn world_to_screen(&self, wx: f32, wy: f32) -> [f32; 2] {
        [wx * self.zoom + self.offset[0], wy * self.zoom + self.offset[1]]
    }

    #[inline]
    pub fn screen_to_world(&self, sx: f32, sy: f32) -> [f32; 2] {
        [
            (sx - self.offset[0]) / self.zoom,
            (sy - self.offset[1]) / self.zoom,
        ]
    }

    /// 平移（屏幕像素增量）。
    #[inline]
    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.offset[0] += dx;
        self.offset[1] += dy;
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

    /// 屏幕矩形（物理像素）覆盖的世界 tile 坐标范围（两端均含）。
    /// 返回 `(tx_min, ty_min, tx_max, ty_max)`。
    ///
    /// 世界区间视为 `[x0, x1)` 半开：右/下边缘恰好落在 tile 边界时不多取一块。
    pub fn visible_tile_range(&self, w: f32, h: f32, tile_size: i32) -> (i32, i32, i32, i32) {
        const EPS: f32 = 1e-4;
        let [x0, y0] = self.screen_to_world(0.0, 0.0);
        let [x1, y1] = self.screen_to_world(w, h);
        let tile_of = |v: f32| (v.floor() as i32).div_euclid(tile_size);
        (
            tile_of(x0),
            tile_of(y0),
            tile_of(x1 - EPS),
            tile_of(y1 - EPS),
        )
    }

    /// 世界像素 → 裁剪空间的 4×4 矩阵（列主序，供 WGSL uniform）。
    /// `viewport` 为渲染目标物理像素尺寸。
    pub fn clip_matrix(&self, viewport_w: f32, viewport_h: f32) -> [[f32; 4]; 4] {
        let sx = 2.0 * self.zoom / viewport_w;
        let sy = -2.0 * self.zoom / viewport_h;
        let tx = 2.0 * self.offset[0] / viewport_w - 1.0;
        let ty = 1.0 - 2.0 * self.offset[1] / viewport_h;
        [
            [sx, 0.0, 0.0, 0.0],
            [0.0, sy, 0.0, 0.0],
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
        };
        // 128×128 视口恰好覆盖 tile (0,0) 与 (1,1)
        let r = v.visible_tile_range(128.0, 128.0, 64);
        assert_eq!(r, (0, 0, 1, 1));
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
}
