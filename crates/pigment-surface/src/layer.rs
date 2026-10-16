//! 图层与图层栈 —— 移植 `_mypaint_ref/lib/layer/`（core/data/group/tree）中
//! 与「平面图层栈 + 显示合成」有关的部分。
//!
//! | 参考源 | 本文件 |
//! |---|---|
//! | `core.py:84-92` `LayerBase` 默认值 | [`Layer::new`] |
//! | `group.py:42-59` `LayerStack`（**index 0 = 最顶层**） | [`LayerStack`] |
//! | `group.py:370-376` `effective_opacity`（不可见=0） | [`Layer::effective_opacity`] |
//! | `tree.py:796-811` `RootLayerStack.get_render_ops`（背景 BLIT 最先，子层自底向上） | [`LayerStack::composite_tile`] |
//! | `tree.py:55-58` 背景默认 (255,255,255) + `_background_visible` | [`LayerStack::background_*`] |
//! | `tiledsurface.py:440-457` `blit_tile_into`（透明源的跳过/清除规则） | [`LayerStack::composite_tile`] |
//!
//! 范围说明（B2）：
//! - 先做**平面栈**；嵌套组（LayerStack 递归 + PUSH/POP 隔离缓冲 +
//!   PASS_THROUGH）记账为后续小项，届时只改本文件的合成循环。
//! - 背景层先支持**纯色**（`BackgroundLayer` 的最常用档）；背景图案档
//!   （repeating tile PNG）记账。
//! - `VectorLayer`/`FallbackBitmapLayer` 不移植（MyPaint 2.0 已弃用建立）。

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};

use crate::color;
use crate::combine::{tile_combine, CombineMode, DEFAULT_MODE};
use crate::PigmentSurface;

/// 单 tile 的 u16 通道数。
const TILE_CHANNELS: usize = TILE_SIZE * TILE_SIZE * 4;

/// 单个图层（`core.py` `LayerBase` 默认值 + `data.PaintingLayer` 的表面）。
#[derive(Clone)]
pub struct Layer {
    pub surface: PigmentSurface,
    /// `core.py:85` `self._name`（默认 `None` → 显示时用 UI 兜底名）。
    pub name: Option<String>,
    /// `core.py:85` 默认 1.0。
    pub opacity: f32,
    /// `core.py:86` 默认 true。
    pub visible: bool,
    /// `core.py:87` 默认 false。
    pub locked: bool,
    /// `core.py:88` 默认 `lib.modes.default_mode()` = SpectralWGM。
    pub mode: CombineMode,
}

impl Default for Layer {
    fn default() -> Self {
        Self::new()
    }
}

impl Layer {
    pub fn new() -> Self {
        Self {
            surface: PigmentSurface::new(),
            name: None,
            opacity: 1.0,
            visible: true,
            locked: false,
            mode: DEFAULT_MODE,
        }
    }

    /// 合成时使用的不透明度：不可见层为 0（`group.py:370-376`）。
    pub fn effective_opacity(&self) -> f32 {
        if self.visible {
            self.opacity
        } else {
            0.0
        }
    }

    /// 图层显示名（无名字时用 UI 兜底名，对应 `layerswindow` 的惯例）。
    pub fn display_name(&self, index: usize) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Layer {}", index + 1))
    }
}

/// 根图层栈（平面版）：`layers[0]` 是**最顶层**（`group.py:44-48`）。
///
/// 背景层按 `tree.py:54-58` 独立保管（不进栈、不受图层操作影响）。
pub struct LayerStack {
    layers: Vec<Layer>,
    /// 当前图层索引（与 `_layers` 同向：0 = 最顶层）。
    current: usize,
    /// 背景色（`tree.py:55` 默认 (255, 255, 255)）。
    background_color: [u8; 3],
    /// `tree.py:58` `_background_visible`。
    background_visible: bool,
}

impl Default for LayerStack {
    fn default() -> Self {
        Self::new()
    }
}

impl LayerStack {
    pub fn new() -> Self {
        Self {
            layers: vec![Layer::new()],
            current: 0,
            background_color: [255, 255, 255],
            background_visible: true,
        }
    }

    // ---- 列表访问（`group.py` 的 list 语义）----

    /// 子层数。
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 按索引取层（0 = 最顶层）。
    #[inline]
    pub fn layer(&self, index: usize) -> &Layer {
        &self.layers[index]
    }

    #[inline]
    pub fn layer_mut(&mut self, index: usize) -> &mut Layer {
        &mut self.layers[index]
    }

    /// 自底向上迭代（显示合成顺序；`group.py:429` `reversed(self._layers)`）。
    pub fn iter_bottom_up(&self) -> impl Iterator<Item = (usize, &Layer)> {
        self.layers.iter().enumerate().rev()
    }

    /// 自顶向下迭代（UI 列表与 `.ora` 保存的条目顺序）。
    pub fn iter_top_down(&self) -> impl Iterator<Item = (usize, &Layer)> {
        self.layers.iter().enumerate()
    }

    // ---- 当前层 ----

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn set_current_index(&mut self, index: usize) {
        self.current = index.min(self.layers.len().saturating_sub(1));
    }

    pub fn current(&self) -> &Layer {
        &self.layers[self.current]
    }

    pub fn current_mut(&mut self) -> &mut Layer {
        let idx = self.current;
        &mut self.layers[idx]
    }

    // ---- 结构操作（`group.py` insert/pop；撤销命令在 pigment-engine 侧做）----

    /// 在 `index` 前插入层并返回实际索引（`group.py:320` `insert` 的钳位语义）。
    pub fn insert(&mut self, index: usize, layer: Layer) -> usize {
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        if self.current >= index {
            self.current += 1;
        }
        index
    }

    /// 移除指定索引的层（`group.py:286` `pop`）。
    ///
    /// 栈不会为空：移除最后一层时补一个新的空层（对应 `layerswindow` 强制
    /// 「至少一层」的行为）。
    pub fn remove(&mut self, index: usize) -> Layer {
        let removed = self.layers.remove(index);
        if self.layers.is_empty() {
            self.layers.push(Layer::new());
            self.current = 0;
        } else {
            if self.current >= index && self.current > 0 {
                self.current -= 1;
            }
            self.current = self.current.min(self.layers.len() - 1);
        }
        removed
    }

    // ---- 背景层（`tree.py:54-58` / `data.py:1023-1060`）----

    pub fn background_color(&self) -> [u8; 3] {
        self.background_color
    }

    pub fn set_background_color(&mut self, rgb: [u8; 3]) {
        self.background_color = rgb;
    }

    pub fn background_visible(&self) -> bool {
        self.background_visible
    }

    pub fn set_background_visible(&mut self, visible: bool) {
        self.background_visible = visible;
    }

    /// 背景的不透明 fix15 tile（`BackgroundLayer`：mode=Normal、opacity=1）。
    fn background_tile(&self) -> Box<TilePixels> {
        let mut tile = empty_tile();
        let [r, g, b] = self.background_color;
        // 背景与载入 PNG 同一条转换路径（EOTF 2.2 + 预乘，alpha 满）
        let px = color::rgba8_to_fix15(r, g, b, 255);
        for row in tile.iter_mut() {
            for p in row.iter_mut() {
                *p = px;
            }
        }
        tile
    }

    /// 把 `(tx, ty)` 处的栈合成结果写入 `dst`（fix15 预乘，长度 = tile 通道数）。
    ///
    /// 返回 `true` 表示结果非空透明块：背景可见，或该坐标存在任一层的 tile。
    /// 语义照 `tree.py:796-811` 的 op 流（背景 BLIT → 子层自底向上）与
    /// `tiledsurface.py:440-457` 的透明源规则。
    pub fn composite_tile(&self, tx: i32, ty: i32, dst: &mut [u16]) -> bool {
        debug_assert_eq!(dst.len(), TILE_CHANNELS);

        // 初始 dst：背景可见 → 不透明背景 tile（dst_has_alpha=false）；
        // 否则全透明（dst_has_alpha=true）。照 `_get_render_background`。
        let mut dst_has_alpha = true;
        let mut has_content = false;
        if self.background_visible {
            let bg = self.background_tile();
            dst.copy_from_slice(bg.as_flattened().as_flattened());
            dst_has_alpha = false;
            has_content = true;
        } else {
            dst.fill(0);
        }

        // 子层自底向上
        for (_, layer) in self.iter_bottom_up() {
            let opacity = layer.effective_opacity();
            let mode = layer.mode;
            match layer.surface.tile(tx, ty) {
                Some(src) => {
                    tile_combine(mode, src.as_flattened().as_flattened(), dst, dst_has_alpha, opacity);
                    dst_has_alpha = true;
                    has_content = true;
                }
                None => {
                    // 透明源规则（tiledsurface.py:443-452）：
                    //  - 模式在零 alpha 下无效果 → 跳过（dst 有 alpha 且
                    //    模式清背景时先清零）
                    //  - 其余（DestIn/DestAtop）→ 用全零源照常 tile_combine
                    if !mode.zero_alpha_has_effect() {
                        if dst_has_alpha && mode.zero_alpha_clears_backdrop() {
                            dst.fill(0);
                        }
                        continue;
                    }
                    let zero = empty_tile();
                    tile_combine(mode, zero.as_flattened().as_flattened(), dst, dst_has_alpha, opacity);
                    dst_has_alpha = true;
                }
            }
        }
        has_content
    }

    /// 合成矩形区域为 RGBA8 显示像素（导出/预览用），按 tile 整块合成。
    ///
    /// `dst` 长度 = `w*h*4`；`with_alpha=false` 时输出不透明 RGB
    /// （alpha 写 255，合成保证不透明）。
    pub fn render_rect_rgba8(&self, rect: (i32, i32, u32, u32), with_alpha: bool, dst: &mut [u8]) {
        let (rx, ry, rw, rh) = rect;
        debug_assert_eq!(dst.len(), (rw * rh * 4) as usize);
        let ts = TILE_SIZE as i32;
        let bg = color::bg_to_linear(self.background_color);
        let mut scratch: Vec<u16> = vec![0; TILE_CHANNELS];
        for y in 0..rh as i32 {
            let py = ry + y;
            for x in 0..rw as i32 {
                let px = rx + x;
                let (tx, ty) = (px.div_euclid(ts), py.div_euclid(ts));
                // 行进方向同一 tile 会重复合成；以正确性优先（导出路径），
                // 显示路径走 GPU 增量上传，不经过这里。
                let composed = self.composite_tile(tx, ty, &mut scratch);
                let in_x = px.rem_euclid(ts) as usize;
                let in_y = py.rem_euclid(ts) as usize;
                let src = &scratch[(in_y * TILE_SIZE + in_x) * 4..][..4];
                let mut p4: [u16; 4] = src.try_into().unwrap();
                if !composed {
                    p4 = [0; 4];
                }
                let o = (y as usize * rw as usize + x as usize) * 4;
                let out = if with_alpha {
                    color::tile_to_rgba8_alpha_single(p4)
                } else {
                    color::tile_to_rgba8_single(p4, self.background_color, bg)
                };
                dst[o..o + 4].copy_from_slice(&out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一块纯色 tile（8 位 → fix15，同载入路径）。
    fn solid_surface(rgb: [u8; 3], a: u8) -> PigmentSurface {
        let mut s = PigmentSurface::new();
        let w = TILE_SIZE as u32;
        let mut px = Vec::with_capacity((w * w * 4) as usize);
        for _ in 0..w * w {
            px.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
        s.load_from_rgba8(&px, 0, 0, w, w);
        s
    }

    #[test]
    fn defaults_match_layer_base() {
        // core.py:84-92
        let l = Layer::new();
        assert_eq!(l.opacity, 1.0);
        assert!(l.visible);
        assert!(!l.locked);
        assert_eq!(l.mode, DEFAULT_MODE);
        assert_eq!(l.effective_opacity(), 1.0);
        // group.py:370-376：不可见 → 0
        let mut l2 = Layer::new();
        l2.visible = false;
        assert_eq!(l2.effective_opacity(), 0.0);
    }

    #[test]
    fn index_zero_is_the_topmost_layer() {
        // group.py:44-48：_layers[0] 是最顶层
        let mut st = LayerStack::new();
        st.layer_mut(0).mode = CombineMode::Normal;
        let mut red = Layer::new();
        red.mode = CombineMode::Normal;
        red.surface = solid_surface([255, 0, 0], 255);
        st.insert(0, red);
        // index 0 = red（最顶），index 1 = 底下的空层
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!([px[0], px[1], px[2]], [255, 0, 0], "顶层红色应盖住背景");
    }

    #[test]
    fn invisible_layer_shows_what_is_below() {
        let mut st = LayerStack::new();
        st.layer_mut(0).mode = CombineMode::Normal;
        let mut blue = Layer::new();
        blue.mode = CombineMode::Normal;
        blue.surface = solid_surface([0, 0, 255], 255);
        st.insert(0, blue);
        st.layer_mut(0).visible = false;
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        // 背景是白色（tree.py:55）
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        assert_eq!([px[0], px[1], px[2]], [255, 255, 255]);
    }

    #[test]
    fn half_opacity_layer_blends_over_background() {
        let mut st = LayerStack::new();
        st.layer_mut(0).mode = CombineMode::Normal;
        let mut red = Layer::new();
        red.mode = CombineMode::Normal;
        red.opacity = 0.5;
        red.surface = solid_surface([255, 0, 0], 255);
        st.insert(0, red);
        let mut dst = vec![0u16; TILE_CHANNELS];
        st.composite_tile(0, 0, &mut dst);
        let px = color::tile_to_rgba8_alpha_single([dst[0], dst[1], dst[2], dst[3]]);
        // 线性域 50% 红 + 50% 白，EOTF 2.2 编码后应在两者之间且偏亮
        assert!(px[0] > 200, "r={}", px[0]);
        assert!(px[1] < 255 && px[1] > 100, "g={}", px[1]);
        assert_eq!(px[1], px[2]);
        assert!(px[3] > 240);
    }

    #[test]
    fn hidden_background_yields_transparency() {
        let mut st = LayerStack::new();
        st.layer_mut(0).mode = CombineMode::Normal;
        st.set_background_visible(false);
        let mut dst = vec![0u16; TILE_CHANNELS];
        assert!(!st.composite_tile(0, 0, &mut dst), "无内容应返回 false");
        assert!(dst.iter().all(|&v| v == 0));
        // 有内容的层仍应出现
        let mut red = Layer::new();
        red.mode = CombineMode::Normal;
        red.surface = solid_surface([255, 0, 0], 255);
        st.insert(0, red);
        let mut dst2 = vec![0u16; TILE_CHANNELS];
        assert!(st.composite_tile(0, 0, &mut dst2));
        assert!(dst2.iter().any(|&v| v != 0));
    }

    #[test]
    fn insert_and_remove_keep_current_sane() {
        let mut st = LayerStack::new(); // [空], current=0
        let l1 = Layer::new();
        assert_eq!(st.insert(0, l1), 0); // [new, 空], current=1
        assert_eq!(st.current_index(), 1);
        st.remove(1); // [new], current=0
        assert_eq!(st.current_index(), 0);
        assert_eq!(st.len(), 1);
        // 移除最后一层 → 自动补空层（不会出现空栈）
        st.remove(0);
        assert_eq!(st.len(), 1);
        assert_eq!(st.current_index(), 0);
    }

    #[test]
    fn render_rect_rgba8_matches_tile_composition() {
        let mut st = LayerStack::new();
        st.layer_mut(0).mode = CombineMode::Normal;
        let mut red = Layer::new();
        red.mode = CombineMode::Normal;
        red.surface = solid_surface([255, 0, 0], 255);
        st.insert(0, red);
        let w = TILE_SIZE as u32;
        let mut buf = vec![0u8; (w * w * 4) as usize];
        st.render_rect_rgba8((0, 0, w, w), true, &mut buf);
        assert_eq!(&buf[0..4], &[255, 0, 0, 255]);
        // 越出内容区一格：应仍是背景白
        let mut buf2 = vec![0u8; 4];
        st.render_rect_rgba8((TILE_SIZE as i32 * 5, 0, 1, 1), true, &mut buf2);
        assert_eq!(&buf2, &[255, 255, 255, 255]);
    }
}
