//! 视图变换：世界像素坐标 ↔ 屏幕像素坐标（平移 + 以锚点缩放）。
//!
//! 约定：`screen = world * zoom + offset`（offset/zoom 均为屏幕物理像素域）。

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
    pub fn zoom_at(&mut self, ax: f32, ay: f32, factor: f32) {
        let before = self.screen_to_world(ax, ay);
        self.zoom = (self.zoom * factor).clamp(0.05, 32.0);
        let after_screen = self.world_to_screen(before[0], before[1]);
        self.offset[0] += ax - after_screen[0];
        self.offset[1] += ay - after_screen[1];
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
}
