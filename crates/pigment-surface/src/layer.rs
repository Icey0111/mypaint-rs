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

use std::sync::atomic::{AtomicU64, Ordering};

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};

use crate::color;
use crate::color::TILE_RGBA8_BYTES;
use crate::combine::{tile_combine, CombineMode, DEFAULT_MODE};
use crate::PigmentSurface;

/// 单 tile 的 u16 通道数。
const TILE_CHANNELS: usize = TILE_SIZE * TILE_SIZE * 4;

/// 图层 id 发生器。MyPaint 的命令持有**图层对象引用**（weakref），与栈序无关；
/// Rust 不能持引用，用单调 id 等价替代（Clone/快照恢复保留原 id）。
static NEXT_LAYER_ID: AtomicU64 = AtomicU64::new(1);
fn next_layer_id() -> u64 {
    NEXT_LAYER_ID.fetch_add(1, Ordering::Relaxed)
}

/// 分配一个新图层 id（`DuplicateLayer` 克隆层用；公开因为引擎层
/// 需要给 COW 克隆出的副本换 id）。
pub fn allocate_layer_id() -> u64 {
    next_layer_id()
}

/// 单个图层（`core.py` `LayerBase` 默认值 + `data.PaintingLayer` 的表面）。
#[derive(Clone)]
pub struct Layer {
    /// 稳定 id：命令按它寻址（等价 MyPaint 的 layer 对象引用）。
    pub id: u64,
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
            id: next_layer_id(),
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

    /// 图层数据外接框（`lib/layer/data.py:314 get_bbox`）：
    /// 已分配 tile 的最小外接矩形，**tile 对齐**；空层返回全 0。
    pub fn data_bbox(&self) -> (i32, i32, i32, i32) {
        let mut coords = self.surface.tiles().map(|(c, _)| c);
        let Some(first) = coords.next() else {
            return (0, 0, 0, 0);
        };
        let (mut x0, mut y0, mut x1, mut y1) = (first.0, first.1, first.0, first.1);
        for (tx, ty) in coords {
            x0 = x0.min(tx);
            y0 = y0.min(ty);
            x1 = x1.max(tx);
            y1 = y1.max(ty);
        }
        let ts = TILE_SIZE as i32;
        (x0 * ts, y0 * ts, (x1 - x0 + 1) * ts, (y1 - y0 + 1) * ts)
    }

    /// **图层**缩略图（`lib/layer/core.py:922 render_thumbnail` →
    /// `tree.py:564 render_layer_preview(layer, bbox, alpha=True)`）。
    ///
    /// 渲染的是本层**自己的 op**（`data.py:376-383`：
    /// `COMPOSITE(surface, mode, opacity)` 落在全透明底上）——所以混合模式与
    /// 不透明度都会体现在缩略图里；solo 只把「不可见」强制成可见
    /// （`data.py:353-358`），**不**重置 mode/opacity。
    ///
    /// `rect` 传 level 0 的模型坐标；减半规则见 [`crate::thumb::render_halved`]。
    pub fn render_thumbnail_rgba8(
        &self,
        rect: (i32, i32, i32, i32),
        level: u32,
    ) -> (Vec<u8>, u32, u32) {
        let mode = self.mode;
        let opacity = self.opacity;
        let surface = &self.surface;
        let mut scratch = vec![0u16; TILE_CHANNELS];
        let src = tile_source(move |(tx, ty), buf| {
            let Some(tile) = surface.tile(tx, ty) else {
                return false;
            };
            scratch.fill(0);
            tile_combine(
                mode,
                tile.as_flattened().as_flattened(),
                &mut scratch,
                true,
                opacity,
            );
            buf.copy_from_slice(&scratch);
            true
        });
        crate::thumb::render_halved(rect, level, crate::thumb::encode_alpha, src)
    }
}

/// 惰性「逐 tile 取源像素」访问器：按需调 `compose` 生成整块 tile 并缓存 4 槽
/// （减半块边长 ≤ 2^4 = 16，最多跨 2×2 个 tile），块内行进方向连续、命中率高。
fn tile_source<'a, F>(mut compose: F) -> impl FnMut(i32, i32) -> [u16; 4] + 'a
where
    F: FnMut((i32, i32), &mut [u16]) -> bool + 'a,
{
    let ts = TILE_SIZE as i32;
    let mut slots: Vec<((i32, i32), bool, Vec<u16>)> = Vec::with_capacity(4);
    move |x: i32, y: i32| {
        let key = (x.div_euclid(ts), y.div_euclid(ts));
        let idx = match slots.iter().position(|(k, _, _)| *k == key) {
            Some(i) => i,
            None => {
                if slots.len() == 4 {
                    slots.remove(0);
                }
                let mut buf = vec![0u16; TILE_CHANNELS];
                let composed = compose(key, &mut buf);
                slots.push((key, composed, buf));
                slots.len() - 1
            }
        };
        let (_, composed, buf) = &slots[idx];
        if !composed {
            return [0, 0, 0, 0];
        }
        let ix = x.rem_euclid(ts) as usize;
        let iy = y.rem_euclid(ts) as usize;
        let o = (iy * TILE_SIZE + ix) * 4;
        buf[o..o + 4].try_into().unwrap()
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
    /// 合成版本号：栈结构/背景/图层属性变化时 +1（tile 内容变化由各层
    /// `surface.clock()` 独立计数，见 [`LayerStack::composite_revision`]）。
    rev: u64,
}

impl Default for LayerStack {
    fn default() -> Self {
        Self::new()
    }
}

/// 克隆共享各层 tile 的 `Arc`（COW），id 原样保留。
impl Clone for LayerStack {
    fn clone(&self) -> Self {
        Self {
            layers: self.layers.clone(),
            current: self.current,
            background_color: self.background_color,
            background_visible: self.background_visible,
            rev: self.rev,
        }
    }
}

impl LayerStack {
    pub fn new() -> Self {
        Self {
            layers: vec![Layer::new()],
            current: 0,
            background_color: [255, 255, 255],
            background_visible: true,
            rev: 0,
        }
    }

    /// 栈/背景/图层属性变化后调用：抬高合成版本号。
    pub fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// 由一组**自顶向下**的层直接构造（`.ora` 载入路径；index 0 = 最顶层）。
    ///
    /// 空列表等价于 [`LayerStack::new`]（补一层空绘画层）。
    pub fn from_layers(layers: Vec<Layer>) -> Self {
        let mut layers = layers;
        if layers.is_empty() {
            layers.push(Layer::new());
        }
        Self {
            layers,
            current: 0,
            background_color: [255, 255, 255],
            background_visible: true,
            rev: 0,
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

    /// 按索引取层（0 = 最顶层）；越界返回 `None`。
    #[inline]
    pub fn try_layer(&self, index: usize) -> Option<&Layer> {
        self.layers.get(index)
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

    /// 把当前层切到 id 对应的那层；返回是否找到（找不到不动）。
    pub fn set_current_by_id(&mut self, id: u64) -> bool {
        match self.index_of_id(id) {
            Some(i) => {
                self.current = i;
                true
            }
            None => false,
        }
    }

    pub fn current(&self) -> &Layer {
        &self.layers[self.current]
    }

    pub fn current_mut(&mut self) -> &mut Layer {
        let idx = self.current;
        &mut self.layers[idx]
    }

    pub fn current_id(&self) -> u64 {
        self.layers[self.current].id
    }

    /// 当前层的 id（栈里找不到时 `None`）。
    pub fn index_of_id(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// 按 id 取层（等价 MyPaint 的按对象引用寻址）。
    pub fn layer_by_id(&self, id: u64) -> Option<&Layer> {
        self.index_of_id(id).map(|i| &self.layers[i])
    }

    pub fn layer_by_id_mut(&mut self, id: u64) -> Option<&mut Layer> {
        let idx = self.index_of_id(id)?;
        Some(&mut self.layers[idx])
    }

    /// 按 id 取该层表面（供命令系统就地改像素）。
    pub fn surface_by_id(&mut self, id: u64) -> Option<&mut PigmentSurface> {
        self.layer_by_id_mut(id).map(|l| &mut l.surface)
    }

    /// 某坐标是否有**笔迹**（任一层有 tile；不含背景）。
    ///
    /// GPU 同步的判据：背景 tile 不上传——面板底色就是文档背景色
    /// （`workspace` 的填充取 [`LayerStack::background_color`]），二者无缝衔接。
    pub fn has_paint_at(&self, tx: i32, ty: i32) -> bool {
        self.layers
            .iter()
            .any(|l| l.surface.tile_entry(tx, ty).is_some())
    }

    /// 某坐标是否需要 GPU 上的合成结果（背景可见，或任一层有 tile）。
    pub fn has_content_at(&self, tx: i32, ty: i32) -> bool {
        self.background_visible
            || self.layers.iter().any(|l| l.surface.tile_entry(tx, ty).is_some())
    }

    /// 合成版本号：任何影响"所有 tile 合成结果"的变化（结构/背景/属性）
    /// 都会抬高它；tile 级内容变化看各层 `surface.clock()`。GPU 同步把
    /// 两者合起来当水位线。
    pub fn composite_revision(&self) -> u64 {
        let mut rev = self.rev;
        for l in &self.layers {
            rev = rev.wrapping_add(l.surface.clock().wrapping_mul(0x9E37_79B9_7F4A_7C15));
        }
        rev
    }

    // ---- 结构操作（`group.py` insert/pop；撤销命令在 pigment-engine 侧做）----

    /// 在 `index` 前插入层并返回实际索引（`group.py:320` `insert` 的钳位语义）。
    pub fn insert(&mut self, index: usize, layer: Layer) -> usize {
        let index = index.min(self.layers.len());
        self.layers.insert(index, layer);
        if self.current >= index {
            self.current += 1;
        }
        self.touch();
        index
    }

    /// 移除指定索引的层（`group.py:286` `pop`）。
    ///
    /// 允许把栈删空：「至少一层」的约束由 RemoveLayer 命令负责（照
    /// `lib/command.py:1040-1055` 的 `CREATE_PAINTING_LAYER_IF_EMPTY`）；
    /// 补层逻辑在命令里，撤销才能精确恢复结构。
    /// 按 id 移除层（返回被移除的层；id 不存在时 `None`）。
    pub fn remove_by_id(&mut self, id: u64) -> Option<Layer> {
        let idx = self.index_of_id(id)?;
        Some(self.remove(idx))
    }

    pub fn remove(&mut self, index: usize) -> Layer {
        let removed = self.layers.remove(index);
        if !self.layers.is_empty() {
            if self.current >= index && self.current > 0 {
                self.current -= 1;
            }
            self.current = self.current.min(self.layers.len() - 1);
        } else {
            self.current = 0;
        }
        self.touch();
        removed
    }

    // ---- 背景层（`tree.py:54-58` / `data.py:1023-1060`）----

    pub fn background_color(&self) -> [u8; 3] {
        self.background_color
    }

    pub fn set_background_color(&mut self, rgb: [u8; 3]) {
        self.background_color = rgb;
        self.touch();
    }

    pub fn background_visible(&self) -> bool {
        self.background_visible
    }

    pub fn set_background_visible(&mut self, visible: bool) {
        self.background_visible = visible;
        self.touch();
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

    /// 合成 `(tx, ty)` 处的 tile 并编码为显示用 RGBA8（**不透明**，GPU 上传用）。
    ///
    /// 背景参与合成（`tree.py` 的背景 BLIT 语义）；背景隐藏时内容直接落在
    /// 纸色上——与现渲染管线的"CPU 预合成、无混合"模型一致。坐标无内容时
    /// 返回 `None`（调用方据此从显存剔除）。
    pub fn composite_tile_rgba8(&self, tx: i32, ty: i32) -> Option<Box<[u8; TILE_RGBA8_BYTES]>> {
        if !self.has_content_at(tx, ty) {
            return None;
        }
        let mut scratch = vec![0u16; TILE_CHANNELS];
        self.composite_tile(tx, ty, &mut scratch);
        let bg = self.background_color;
        let bg_lin = color::bg_to_linear(bg);
        let mut out = Box::new([0u8; TILE_RGBA8_BYTES]);
        for i in 0..TILE_SIZE * TILE_SIZE {
            let px: [u16; 4] = scratch[i * 4..i * 4 + 4].try_into().unwrap();
            out[i * 4..i * 4 + 4].copy_from_slice(&color::tile_to_rgba8_single(px, bg, bg_lin));
        }
        Some(out)
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
        // 行进方向缓存：同一 tile 只合成一次（导出 3456×3008 这种大画幅时，
        // 每像素重合成整 tile 是 64×64 倍的无谓开销）。
        let mut last: (i32, i32) = (i32::MIN, i32::MIN);
        let mut last_composed = false;
        for y in 0..rh as i32 {
            let py = ry + y;
            let row_ty = py.div_euclid(ts);
            let in_y = py.rem_euclid(ts) as usize;
            for x in 0..rw as i32 {
                let px = rx + x;
                let tx = px.div_euclid(ts);
                if (tx, row_ty) != last {
                    last = (tx, row_ty);
                    last_composed = self.composite_tile(tx, row_ty, &mut scratch);
                }
                let composed = last_composed;
                let in_x = px.rem_euclid(ts) as usize;
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

    /// **栈**缩略图渲染（`root_stack.render_thumbnail` 的 fix15 版）：
    /// 合成（含背景）后按 `level` 级固定点减半，输出不透明 RGBA8。
    ///
    /// 与 [`LayerStack::render_rect_rgba8`] 同一套合成语义（背景 + 各层模式），
    /// 差别只在于**在 fix15 预乘域做 mipmap 平均**（MyPaint 的 mipmap 就在
    /// 15 位域），最后一步才转 8 位。
    pub fn render_thumbnail_rgba8(
        &self,
        rect: (i32, i32, i32, i32),
        level: u32,
    ) -> (Vec<u8>, u32, u32) {
        let bg = self.background_color;
        let src = tile_source(|(tx, ty), buf| self.composite_tile(tx, ty, buf));
        crate::thumb::render_halved(rect, level, |p| crate::thumb::encode_opaque(p, bg), src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::TiledSurface;

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

    /// 图层缩略图：减半在 fix15 预乘域做，且**每级每通道各自整除再相加**
    /// （`pixops.cpp:39` 的 `v0/4 + v1/4 + v2/4 + v3/4`）。
    #[test]
    fn layer_thumbnail_halves_in_fix15_space() {
        let mut l = Layer::new();
        l.mode = CombineMode::Normal; // 直接透传，便于逐位比对
        // 2×2 全不透明，R 通道取 1/2/3/4（fix15 直接写，跳过 8 位量化）
        let vals: [[u16; 4]; 4] = [
            [1, 0, 0, 1 << 15],
            [2, 0, 0, 1 << 15],
            [3, 0, 0, 1 << 15],
            [4, 0, 0, 1 << 15],
        ];
        {
            let t = l.surface.tile_request_start(0, 0);
            t[0][0] = vals[0];
            t[0][1] = vals[1];
            t[1][0] = vals[2];
            t[1][1] = vals[3];
            l.surface.tile_request_end(0, 0);
        }
        // 64×64 的 bbox 取 level 1 → 32×32
        let (px, w, h) = l.render_thumbnail_rgba8((0, 0, 64, 64), 1);
        assert_eq!((w, h), (32, 32));

        // 逐位期望：各自 /4 后相加（1/4 + 2/4 + 3/4 + 4/4 = 0+0+0+1 = 1），
        // 不是 (1+2+3+4)/4 = 2
        let mut avg = [0u16; 4];
        for c in 0..4 {
            avg[c] = vals[0][c] / 4 + vals[1][c] / 4 + vals[2][c] / 4 + vals[3][c] / 4;
        }
        let expected = color::tile_to_rgba8_alpha_single(avg);
        assert_eq!(avg[0], 1, "先各自整除再相加");
        assert_eq!(&px[0..4], &expected[..]);
        // 块外（1,1 那个 2×2 只占左上角）应为全透明
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
    }

    /// 块边界按 **mipmap 网格**对齐（源码先 `x //= 2` 再交给 mipmap 层）。
    #[test]
    fn layer_thumbnail_blocks_align_to_the_mipmap_grid() {
        let mut l = Layer::new();
        l.mode = CombineMode::Normal;
        {
            let t = l.surface.tile_request_start(0, 0);
            t[0][0] = [1 << 15, 1 << 15, 1 << 15, 1 << 15];
            l.surface.tile_request_end(0, 0);
        }
        let (px, w, h) = l.render_thumbnail_rgba8((0, 0, 64, 64), 1);
        assert_eq!((w, h), (32, 32));
        // (0,0) 的 2×2 块含 1 个不透明像素 → 平均 alpha = 32768/4 = 8192
        assert_eq!(px[3], (8192.0f32 / 32768.0 * 255.0).round() as u8);
        assert_eq!(px[0], 255, "去预乘后仍是满强度白");
        assert_eq!(&px[4..8], &[0, 0, 0, 0], "相邻块全空");
    }

    /// 缩略图走**本层自己的 op**：不透明度会体现在缩略图里
    /// （`data.py:376-383` 的 `COMPOSITE(surface, mode, opacity)`）。
    #[test]
    fn layer_thumbnail_applies_own_opacity() {
        let mut l = Layer::new();
        l.mode = CombineMode::Normal;
        l.surface = solid_surface([255, 0, 0], 255);
        let (full, _, _) = l.render_thumbnail_rgba8((0, 0, 64, 64), 0);
        assert_eq!(&full[0..4], &[255, 0, 0, 255]);

        l.opacity = 0.5;
        let (half, _, _) = l.render_thumbnail_rgba8((0, 0, 64, 64), 0);
        assert!(half[3] < 250 && half[3] > 100, "alpha 落到约一半：{}", half[3]);
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
        // 照源码语义：允许删空（补层由 RemoveLayer 命令负责）
        st.remove(0);
        assert_eq!(st.len(), 0);
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
