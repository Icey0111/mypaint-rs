//! pigment-surface —— pigment 项目的自家 tile 表面后端（无限画布存储层）。
//!
//! 实现 hokusai 的 [`TiledSurface`] trait。混合算法（`draw_dab` / `get_color` /
//! `get_color_pigment`）**刻意用 hokusai-core 的默认实现**，保证与参考后端
//! `MemSurface` 逐像素一致（行为对等由 `tests/parity.rs` 守住）。
//!
//! 唯一的覆写是 [`symmetry`]：libmypaint 把对称展开放在表面自己的 `draw_dab`
//! 里（`mypaint-tiled-surface.c:690-800`），所以这里也照做 —— `draw_dab` =
//! 默认实现画原始 dab + 对称 pass。**对称关闭时逐像素与 MemSurface 全等**，
//! 这条由 `tests/symmetry.rs` 单独守（见 `symmetry_off_is_bit_identical_to_reference`）。
//!
//! 相对 MemSurface 的增量（为 M2 wgpu 渲染层铺路）：
//!
//! 1. **内容版本号**：每个 tile 携带单调递增的 `revision`。GPU 纹理镜像
//!    只需记录「已上传的 revision」，`revision > uploaded` 即需重传——
//!    增量上传不需要 diff 像素。
//! 2. **LRU 元数据**：`last_touched` 记录最近访问帧号，为显存/内存驻留
//!    淘汰策略（M2 的 LRU 显存驻留）提供依据。
//! 3. **原子脏区**：`begin_atomic` / `end_atomic` 收集一次笔画内被修改的
//!    tile 列表（与 libmypaint 的 `tile_request` 语义对齐）。
//! 4. **写时复制快照**：`save_snapshot` / `load_snapshot` 是 M4 撤销体系的
//!    基石（见下）。
//!
//! # 快照（照搬 MyPaint `lib/tiledsurface.py:461-494`）
//!
//! MyPaint 的做法是：`save_snapshot()` 把全部 tile 标记为 `readonly` 并
//! **浅拷贝** tiledict（共享 tile 对象），之后任何写借出遇到 `readonly`
//! 的 tile 就先复制一份再写——即经典的 copy-on-write；`load_snapshot()`
//! 把 tiledict 换回快照那份，靠**对象同一性**的对称差算出变化的 tile 并
//! 通知观察者（未变的 tile 对象被两个 dict 共享，天然"无变化"）。
//!
//! Rust 侧用 `Arc<TilePixels>` + [`Arc::make_mut`] 表达同一件事：
//!
//! - `save_snapshot` 浅拷贝一张坐标→`Arc` 的表（不复制像素）；
//! - 写借出时 `Arc::make_mut` 在**引用计数 > 1**（即与某个快照共享）时
//!   自动复制像素、脱离共享，计数为 1 时零开销——对应 MyPaint 的
//!   `readonly` 分支；
//! - `load_snapshot` 用 `Arc::ptr_eq` 判"对象是否同一个"，等价于 MyPaint
//!   的 `set(tiledict.items())` 对称差。
//!
//! 因此快照成本与撤销内存都只落在「真正被改过的 tile」上。
//!
//! 扩展点（暂未启用）：tooth 纹理沉积（M6 超越点）将在本 crate 覆盖
//! `draw_dab` 时注入高度图系数；届时以 parity 测试的对照快照做回归门禁。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};
use hokusai::{Dab, TiledSurface};

pub mod color;
pub mod combine;
pub mod layer;
pub mod png;
pub mod symmetry;
pub mod thumb;

pub use layer::{Layer, LayerStack};
pub use symmetry::{SymmetryData, SymmetryState, SymmetryType, Transform};

/// 单个 tile：像素数据 + 同步/淘汰元数据。
///
/// `pixels` 是 `Arc`：与快照共享时写借出自动复制（见模块文档的写时复制一节）。
pub struct Tile {
    pixels: Arc<TilePixels>,
    /// 内容版本号：每次「写意图」借出（`tile_request_start`）时递增。
    /// 保守策略——借出即视为可能脏，宁可多传不可漏传。
    revision: u64,
    /// 最近访问的全局时钟值（LRU 依据）。
    last_touched: u64,
}

impl Tile {
    fn empty() -> Self {
        Self {
            pixels: Arc::new(*empty_tile()),
            revision: 0,
            last_touched: 0,
        }
    }

    #[inline]
    pub fn pixels(&self) -> &TilePixels {
        &self.pixels
    }

    /// 内容版本号（0 = 从未写入）。
    #[inline]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 最近访问帧号。
    #[inline]
    pub fn last_touched(&self) -> u64 {
        self.last_touched
    }
}

/// 表面快照（不可变）：坐标 → 共享像素。
///
/// 只在 Undo/Redo（`lib/command.py:Brushwork`）与表面复制中使用，
/// 对应 MyPaint 的 `_SurfaceSnapshot`（`lib/tiledsurface.py:94`）。
#[derive(Clone, Default)]
pub struct SurfaceSnapshot {
    tiles: HashMap<(i32, i32), Arc<TilePixels>>,
}

impl SurfaceSnapshot {
    /// 快照里的 tile 数。
    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// 快照像素数据的内存占用（只统计**独占**的 tile，共享部分不计）。
    pub fn memory_bytes(&self) -> usize {
        self.tiles
            .values()
            .filter(|px| Arc::strong_count(px) == 1)
            .count()
            * TILE_SIZE
            * TILE_SIZE
            * 4
            * 2
    }

    /// 只读读取某个 tile（不存在则 `None`）。
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|px| &**px)
    }

    /// 遍历快照持有的 tile 坐标。
    #[inline]
    pub fn coords(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.tiles.keys().copied()
    }
}

/// `load_snapshot` 的结果：哪些 tile 需要重新上传、哪些需要从显存剔除。
///
/// MyPaint 用同一个对称差去 `notify_observers(*bbox)`（`tiledsurface.py:492`）；
/// 我们的渲染层是"显存镜像"模型，所以除了脏 tile 还要知道**被删掉**的 tile，
/// 否则撤销到较早状态后，显存里会残留已经不存在的 tile（画布漏出旧内容）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SnapshotDiff {
    /// 内容变化（含新增）的 tile，需要重新上传。
    pub changed: Vec<(i32, i32)>,
    /// 快照中不存在、需从显存剔除的 tile。
    pub removed: Vec<(i32, i32)>,
}

impl SnapshotDiff {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// 无限画布：`HashMap<(tx, ty), Tile>` 稀疏存储，画到哪里分配到哪里。
///
/// 单 tile 为 64×64 RGBA fix15（预乘 alpha、线性 sRGB），32 KiB，
/// 与 libmypaint / hokusai 的 tile 几何完全一致。
#[derive(Default)]
pub struct PigmentSurface {
    tiles: HashMap<(i32, i32), Tile>,
    /// 原子窗口内被写借出的 tile（`begin_atomic` ~ `end_atomic`）。
    dirty: HashSet<(i32, i32)>,
    in_atomic: bool,
    /// 全局时钟：revision 与 last_touched 的共同来源。
    clock: u64,
    /// 对称绘制状态（`mypaint-tiled-surface.h:46` 的 `symmetry_data`）。
    ///
    /// 挂在表面上而不是笔刷引擎上，是 libmypaint 的做法：一次 `draw_dab`
    /// 由表面自己展开成 N 个 dab（`mypaint-tiled-surface.c:690-800`），
    /// 见 [`symmetry`] 模块。
    symmetry: SymmetryData,
}

/// 克隆：与快照一样按 tile 共享 `Arc`（写借出时 COW 自动分离）。
impl Clone for PigmentSurface {
    fn clone(&self) -> Self {
        Self {
            tiles: self
                .tiles
                .iter()
                .map(|(&coord, t)| (coord, Tile { pixels: Arc::clone(&t.pixels), revision: t.revision, last_touched: t.last_touched }))
                .collect(),
            dirty: HashSet::new(),
            in_atomic: false,
            clock: self.clock,
            symmetry: self.symmetry.clone(),
        }
    }
}

impl PigmentSurface {
    pub fn new() -> Self {
        Self::default()
    }

    /// 只读访问 tile 像素（不存在则 `None`）。
    #[inline]
    pub fn tile(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tiles.get(&(tx, ty)).map(|t| t.pixels())
    }

    /// 只读访问 tile（含元数据）。
    #[inline]
    pub fn tile_entry(&self, tx: i32, ty: i32) -> Option<&Tile> {
        self.tiles.get(&(tx, ty))
    }

    /// 已分配 tile 数。
    #[inline]
    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// 画布占用的内存字节数（每 tile 32 KiB）。
    #[inline]
    pub fn memory_bytes(&self) -> usize {
        self.tiles.len() * TILE_SIZE * TILE_SIZE * 4 * 2
    }

    /// 遍历全部已分配 tile（供整图上传 / 拼合导出）。
    #[inline]
    pub fn tiles(&self) -> impl Iterator<Item = ((i32, i32), &Tile)> {
        self.tiles.iter().map(|(&coord, t)| (coord, t))
    }

    /// 返回 `revision > since` 的 tile 坐标列表。
    ///
    /// GPU 纹理镜像的增量同步入口：渲染层记录上次同步到的全局时钟，
    /// 每帧调用本方法取回需要重新上传的 tile。
    pub fn changed_since(&self, since: u64) -> Vec<(i32, i32)> {
        self.tiles
            .iter()
            .filter(|(_, t)| t.revision > since)
            .map(|(&coord, _)| coord)
            .collect()
    }

    /// 当前全局时钟值（GPU 侧完成一次全量同步后记录此值）。
    #[inline]
    pub fn clock(&self) -> u64 {
        self.clock
    }

    // ---- 快照（照搬 `lib/tiledsurface.py:461-494`）----

    /// 创建并返回表面快照（`tiledsurface.py:461`）。
    ///
    /// 浅拷贝：像素仍与表面共享（`Arc` 计数 +1），只有之后的**写借出**
    /// 才会真正复制被碰到的那几个 tile。等价于 MyPaint 的
    /// `for t in itervalues(self.tiledict): t.readonly = True` + dict 浅拷贝。
    pub fn save_snapshot(&self) -> SurfaceSnapshot {
        SurfaceSnapshot {
            tiles: self
                .tiles
                .iter()
                .map(|(&coord, t)| (coord, Arc::clone(&t.pixels)))
                .collect(),
        }
    }

    /// 加载快照，替换内部 tiledict（`tiledsurface.py:475`）。
    ///
    /// 返回需要同步到显存的差异；内容未变的 tile 会**保留原 `Tile`**
    /// （revision 不变 → 显存无需重传），变化的 tile 拿到新 revision。
    pub fn load_snapshot(&mut self, sshot: &SurfaceSnapshot) -> SnapshotDiff {
        let mut diff = SnapshotDiff::default();
        // 与 MyPaint 的 `if d == self.tiledict: return` 同理：同一张快照
        // 重复加载（典型场景：redo 后再 undo）不该产生任何重传。
        if self.tiles.len() == sshot.tiles.len()
            && self
                .tiles
                .iter()
                .all(|(c, t)| sshot.tiles.get(c).is_some_and(|px| Arc::ptr_eq(&t.pixels, px)))
        {
            return diff;
        }

        self.clock += 1;
        let clock = self.clock;
        let old = std::mem::take(&mut self.tiles);
        let mut tiles = HashMap::with_capacity(sshot.tiles.len());
        for (&coord, px) in &sshot.tiles {
            let tile = match old.get(&coord) {
                // 同一个像素对象 → 内容未变（MyPaint 的集合对称差不含它）
                Some(t) if Arc::ptr_eq(&t.pixels, px) => Tile {
                    pixels: Arc::clone(px),
                    revision: t.revision,
                    last_touched: clock,
                },
                _ => {
                    diff.changed.push(coord);
                    Tile {
                        pixels: Arc::clone(px),
                        revision: clock,
                        last_touched: clock,
                    }
                }
            };
            tiles.insert(coord, tile);
        }
        // 快照里没有、当前却有的 tile：撤销到较早状态时被"删掉"的内容
        diff.removed.extend(
            old.keys()
                .filter(|c| !sshot.tiles.contains_key(c))
                .copied(),
        );
        self.tiles = tiles;
        diff.changed.sort_unstable();
        diff.removed.sort_unstable();
        diff
    }

    /// 用另一个表面的内容替换自身（`tiledsurface.py:498` 的 `load_from_surface`）。
    pub fn load_from_surface(&mut self, other: &PigmentSurface) -> SnapshotDiff {
        let sshot = other.save_snapshot();
        self.load_snapshot(&sshot)
    }

    // ---- 文档 I/O（照搬 `lib/surface.py` 的 TileBlittable 层）----

    /// 把 `rect` 指定的矩形渲染成 RGBA8 像素（行主序、每像素 4 字节）。
    ///
    /// `rect` 是绝对模型坐标 `(x, y, w, h)`；`alpha = false` 时把内容合成到
    /// `bg` 底色上并输出不透明像素，`true` 时保留透明。
    ///
    /// 与 `lib/surface.py:save_as_png` 一致：**空矩形退化成 1×1**，这样空文档
    /// 也能写出一个合法图像（源码注释：「Save at least one tile to allow empty
    /// docs to be written」）。
    pub fn render_rect_rgba8(&self, rect: (i32, i32, i32, i32), alpha: bool, bg: [u8; 3]) -> Vec<u8> {
        let (x, y, mut w, mut h) = rect;
        if w == 0 || h == 0 {
            // 照源码：`if w == 0 or h == 0: x, y, w, h = (0, 0, 1, 1)`
            w = 1;
            h = 1;
        }
        let w = w.max(0) as usize;
        let h = h.max(0) as usize;
        let mut out = vec![0u8; w * h * 4];
        // 底色只解码一次（线性域合成）
        let bg_lin = color::bg_to_linear(bg);
        for row in 0..h {
            let wy = y + row as i32;
            let ty = wy.div_euclid(TILE_SIZE as i32);
            let iy = wy.rem_euclid(TILE_SIZE as i32) as usize;
            for col in 0..w {
                let wx = x + col as i32;
                let tx = wx.div_euclid(TILE_SIZE as i32);
                let ix = wx.rem_euclid(TILE_SIZE as i32) as usize;
                let o = (row * w + col) * 4;
                match self.tiles.get(&(tx, ty)) {
                    None => {
                        // 未分配 = 空：合成模式下写底色，alpha 模式写全透明
                        if alpha {
                            out[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
                        } else {
                            out[o..o + 3].copy_from_slice(&bg);
                            out[o + 3] = 255;
                        }
                    }
                    Some(tile) => {
                        let p = tile.pixels[iy][ix];
                        if alpha {
                            let px = color::tile_to_rgba8_alpha_single(p);
                            out[o..o + 4].copy_from_slice(&px);
                        } else {
                            let px = color::tile_to_rgba8_single(p, bg, bg_lin);
                            out[o..o + 4].copy_from_slice(&px);
                        }
                    }
                }
            }
        }
        out
    }

    /// 把 `rect` 写成 PNG（`lib/surface.py:220` 的 `save_as_png`）。    ///
    /// 返回实际写出的矩形 `(x, y, w, h)` —— 空表面会被退化成 1×1，
    /// 调用方需要知道真正落盘的尺寸（ORA 的 stack.xml 要用）。
    pub fn save_as_png(
        &self,
        filename: impl AsRef<Path>,
        rect: (i32, i32, i32, i32),
        alpha: bool,
        bg: [u8; 3],
    ) -> Result<(i32, i32, i32, i32), String> {
        let (x, y, mut w, mut h) = rect;
        if w == 0 || h == 0 {
            w = 1;
            h = 1;
        }
        png::write_rgba8_png(filename.as_ref(), self.render_rect_rgba8(rect, alpha, bg), w as u32, h as u32)?;
        Ok((x, y, w, h))
    }

    /// 用解码后的 RGBA8 像素替换表面内容（`load_layer_from_png` 的核心）。
    ///
    /// `(x, y)` 是内容左上角的模型坐标；像素按 tile 切分写入。
    /// 与 MyPaint 一致：**整层替换**，调用方应先 `clear()`。
    /// 把一块已经处于 fix15 预乘格式的 tile 原样写入表面（MergeLayerDown 用）。
    ///
    /// 与写借出路径一致地分配缺 tile、抬 revision。长度必须是整 tile。
    pub fn paste_tile(&mut self, tx: i32, ty: i32, pixels: &[u16]) {
        assert_eq!(pixels.len(), TILE_SIZE * TILE_SIZE * 4);
        self.clock += 1;
        let entry = self.tiles.entry((tx, ty)).or_insert_with(|| Tile {
            pixels: Arc::new(*empty_tile()),
            revision: 0,
            last_touched: 0,
        });
        let px = Arc::make_mut(&mut entry.pixels);
        let flat = px.as_flattened_mut().as_flattened_mut();
        flat.copy_from_slice(pixels);
        entry.revision = self.clock;
        entry.last_touched = self.clock;
    }

    pub fn load_from_rgba8(&mut self, pixels: &[u8], x: i32, y: i32, w: u32, h: u32) {
        let (w, h) = (w as i32, h as i32);
        let ts = TILE_SIZE as i32;
        // 行内按 tile 分段处理：一段只做一次 tile 请求与 fix15 转换缓冲，
        // 避免 276M 像素级别的逐像素 div_euclid + 哈希查找（载入整份
        // fill_outlines.ora 时这是分钟级 → 秒级的差别）。
        let mut seg: Vec<[u16; 4]> = Vec::with_capacity(TILE_SIZE as usize);
        for row in 0..h {
            let wy = y + row;
            let ty = wy.div_euclid(ts);
            let iy = wy.rem_euclid(ts) as usize;
            let mut col = 0;
            while col < w {
                let tx = (x + col).div_euclid(ts);
                let seg_len = (ts - (x + col).rem_euclid(ts)).min(w - col);
                let ix0 = (x + col).rem_euclid(ts) as usize;
                // 转换本段（跳过全透明像素 —— 不必分配 tile，与 MyPaint 一致）
                seg.clear();
                let mut any = false;
                for c in 0..seg_len {
                    let o = ((row as usize) * (w as usize) + (col + c) as usize) * 4;
                    let px = color::rgba8_to_fix15(pixels[o], pixels[o + 1], pixels[o + 2], pixels[o + 3]);
                    any |= px[3] != 0;
                    seg.push(px);
                }
                if any {
                    let tile = self.tile_request_start(tx, ty);
                    for (c, px) in seg.iter().enumerate() {
                        if px[3] == 0 {
                            continue;
                        }
                        tile[iy][ix0 + c] = *px;
                    }
                    self.tile_request_end(tx, ty);
                }
                col += seg_len;
            }
        }
    }

    /// 从 PNG 读取并载入表面（`load_png` → `load_layer_from_png`）。
    ///
    /// 返回内容外接矩形 `(x, y, w, h)`，与 `load_layer_from_png` 的返回值
    /// 语义一致（供 `Document` 设画框用）。
    pub fn load_from_png(&mut self, filename: impl AsRef<Path>, x: i32, y: i32) -> Result<(i32, i32, i32, i32), String> {
        let (px, w, h) = png::read_rgba8_png(filename.as_ref())?;
        self.load_from_rgba8(&px, x, y, w, h);
        Ok((x, y, w as i32, h as i32))
    }
}

impl TiledSurface for PigmentSurface {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.clock += 1;
        let clock = self.clock;
        if self.in_atomic {
            self.dirty.insert((tx, ty));
        }
        let tile = self.tiles.entry((tx, ty)).or_insert_with(Tile::empty);
        tile.revision = clock;
        tile.last_touched = clock;
        // 写时复制：与快照共享像素（引用计数 > 1）时先复制一份再写，
        // 对应 MyPaint `tile_request` 里遇到 `tile.readonly` 的复制分支
        // （`tiledsurface.py:280-291`）。独占时零开销。
        Arc::make_mut(&mut tile.pixels)
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tile(tx, ty)
    }

    fn begin_atomic(&mut self) {
        // 对称矩阵在这里重算：源码的 `mypaint_tiled_surface_begin_atomic`
        // 做的就是 `mypaint_update_symmetry_state(&self->symmetry_data)`
        // （`mypaint-tiled-surface.c:94-99`），而 `begin_atomic` 只在"一笔的开头"
        // 被调用，所以重算永远落在 dab 热路径之外。
        self.symmetry.update();
        self.in_atomic = true;
        self.dirty.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.in_atomic = false;
        self.dirty.drain().collect()
    }

    /// 一次 dab = 原始 dab + 对称展开（`mypaint-tiled-surface.c:690-800`）。
    ///
    /// libmypaint 把对称放在**表面**的 `draw_dab` 里，所以笔刷引擎照旧一个
    /// 事件一个 dab；没开启对称时这里就是原来的默认实现，一个像素都不差。
    fn draw_dab(&mut self, dab: &Dab) -> bool {
        self.draw_dab_with_symmetry(dab)
    }

    // get_color / get_color_pigment：全部走 hokusai-core 默认实现，
    // 与 MemSurface 保持逐像素一致。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(surface: &mut PigmentSurface, tx: i32, ty: i32, v: u16) {
        let t = surface.tile_request_start(tx, ty);
        t[0][0] = [v, v, v, 1 << 15];
        surface.tile_request_end(tx, ty);
    }

    fn pixel(surface: &PigmentSurface, tx: i32, ty: i32) -> Option<[u16; 4]> {
        surface.tile(tx, ty).map(|t| t[0][0])
    }

    #[test]
    fn lends_records_dirty_and_stamps_metadata() {
        let mut s = PigmentSurface::new();
        s.begin_atomic();
        {
            let t = s.tile_request_start(2, -1);
            t[0][0] = [1, 2, 3, 4];
            s.tile_request_end(2, -1);
        }
        let dirty = s.end_atomic();
        assert_eq!(dirty, vec![(2, -1)]);

        let tile = s.tile_entry(2, -1).unwrap();
        assert_eq!(tile.pixels()[0][0], [1, 2, 3, 4]);
        assert!(tile.revision() > 0);
        assert_eq!(tile.revision(), tile.last_touched());
        assert_eq!(s.tile(0, 0), None);
        assert_eq!(s.tile_count(), 1);
        assert_eq!(s.memory_bytes(), 32 * 1024);
    }

    #[test]
    fn revision_is_monotonic_and_changed_since_filters() {
        let mut s = PigmentSurface::new();
        let sync_point = s.clock();

        s.tile_request_start(0, 0);
        s.tile_request_start(1, 0);
        s.tile_request_end(0, 0);
        s.tile_request_end(1, 0);

        let mut changed = s.changed_since(sync_point);
        changed.sort();
        assert_eq!(changed, vec![(0, 0), (1, 0)]);

        let after_first_writes = s.clock();
        // 只重写 (0,0)：changed_since 应只剩它
        s.tile_request_start(0, 0);
        s.tile_request_end(0, 0);
        assert_eq!(s.changed_since(after_first_writes), vec![(0, 0)]);
        // 重复借出仍递增（保守脏策略）
        let r1 = s.tile_entry(1, 0).unwrap().revision();
        assert!(r1 < s.tile_entry(0, 0).unwrap().revision());
    }

    /// 快照的核心契约：**之后的写入不得污染快照**（写时复制）。
    #[test]
    fn snapshot_survives_later_writes() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 111);
        let sshot = s.save_snapshot();

        write(&mut s, 0, 0, 222);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 222, "表面应写入新值");
        assert_eq!(
            sshot.tile(0, 0).unwrap()[0][0][0],
            111,
            "快照必须保留旧值（写时复制）"
        );

        // 未触碰的 tile 不复制：像素对象仍与快照同一个
        write(&mut s, 5, 5, 9);
        let s2 = s.save_snapshot();
        assert!(s2.tile(0, 0).is_some() && sshot.tile(0, 0).is_some());

        // 回滚
        s.load_snapshot(&sshot);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 111, "回滚应恢复旧值");
    }

    /// 多张快照共存（undo + redo 两份状态）各自独立。
    #[test]
    fn multiple_snapshots_coexist() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let a = s.save_snapshot();
        write(&mut s, 0, 0, 2);
        let b = s.save_snapshot();
        write(&mut s, 0, 0, 3);

        s.load_snapshot(&b);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 2);
        s.load_snapshot(&a);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 1);
        s.load_snapshot(&b);
        assert_eq!(pixel(&s, 0, 0).unwrap()[0], 2, "两次加载互不影响");
    }

    /// 加载快照要报告差异：变化的 tile 需重传，缺席的 tile 需从显存剔除。
    #[test]
    fn load_snapshot_reports_changed_and_removed() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let sshot = s.save_snapshot();

        write(&mut s, 0, 0, 2); // 变化
        write(&mut s, 9, 9, 3); // 新增（快照里没有）

        let diff = s.load_snapshot(&sshot);
        assert_eq!(diff.changed, vec![(0, 0)], "恢复后 (0,0) 内容变了");
        assert_eq!(diff.removed, vec![(9, 9)], "(9,9) 不在快照里 → 需剔除");
    }

    /// 内容未变的 tile 保留原 revision：显存无需重传。
    #[test]
    fn unchanged_tiles_keep_revision() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        write(&mut s, 1, 0, 7);
        let sshot = s.save_snapshot();
        let before = s.tile_entry(0, 0).unwrap().revision();

        // 只改 (1,0)，再撤销 —— (0,0) 两个状态完全一致
        write(&mut s, 1, 0, 8);
        let diff = s.load_snapshot(&sshot);
        assert_eq!(diff.changed, vec![(1, 0)]);
        assert_eq!(
            s.tile_entry(0, 0).unwrap().revision(),
            before,
            "未变 tile 的 revision 不该动，否则会白白重传"
        );
        assert!(s.tile_entry(1, 0).unwrap().revision() > before);
    }

    /// 重复加载同一张快照是幂等的（redo → undo → redo 的老路径）。
    #[test]
    fn reloading_same_snapshot_is_idempotent() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let sshot = s.save_snapshot();
        let diff = s.load_snapshot(&sshot);
        // 首次加载：状态本来就一致（像素对象同一个）→ 无需任何同步
        assert!(diff.is_empty());
        let rev = s.tile_entry(0, 0).unwrap().revision();
        assert!(s.load_snapshot(&sshot).is_empty());
        assert_eq!(s.tile_entry(0, 0).unwrap().revision(), rev, "revision 不动");
    }

    /// 写时复制只在共享时发生：独占 tile 的像素对象原地复用。
    #[test]
    fn copy_on_write_only_when_shared() {
        let mut s = PigmentSurface::new();
        write(&mut s, 0, 0, 1);
        let lone = Arc::as_ptr(&s.tiles[&(0, 0)].pixels);
        write(&mut s, 0, 0, 2);
        assert_eq!(
            Arc::as_ptr(&s.tiles[&(0, 0)].pixels),
            lone,
            "独占写入不该复制像素"
        );

        let _sshot = s.save_snapshot();
        let shared = Arc::as_ptr(&s.tiles[&(0, 0)].pixels);
        write(&mut s, 0, 0, 3);
        assert_ne!(
            Arc::as_ptr(&s.tiles[&(0, 0)].pixels),
            shared,
            "与快照共享时必须复制像素"
        );
    }

    /// 空表面快照 → 恢复后应把画布清空（document.clear 与 redo 的路径）。
    #[test]
    fn loading_empty_snapshot_clears_surface() {
        let mut s = PigmentSurface::new();
        let empty = s.save_snapshot();
        write(&mut s, 0, 0, 1);
        write(&mut s, 1, 1, 1);
        let diff = s.load_snapshot(&empty);
        assert_eq!(diff.removed, vec![(0, 0), (1, 1)]);
        assert_eq!(s.tile_count(), 0);
    }

    #[test]
    fn load_from_surface_copies_content() {
        let mut a = PigmentSurface::new();
        write(&mut a, 3, 4, 5);
        let mut b = PigmentSurface::new();
        let diff = b.load_from_surface(&a);
        assert_eq!(b.tile(3, 4).unwrap()[0][0][0], 5);
        assert_eq!(diff.changed, vec![(3, 4)]);
        // 复制是写时复制语义：改 a 不影响 b
        write(&mut a, 3, 4, 6);
        assert_eq!(b.tile(3, 4).unwrap()[0][0][0], 5);
    }
}
