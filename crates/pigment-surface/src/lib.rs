//! pigment-surface —— pigment 项目的自家 tile 表面后端（无限画布存储层）。
//!
//! 实现 hokusai 的 [`TiledSurface`] trait。混合算法（`draw_dab` / `get_color` /
//! `get_color_pigment`）**刻意全部使用 hokusai-core 的默认实现**，保证与
//! 参考后端 `MemSurface` 逐像素一致（行为对等由 `tests/parity.rs` 守住）。
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
//!
//! 扩展点（暂未启用）：tooth 纹理沉积（M6 超越点）将在本 crate 覆盖
//! `draw_dab` 时注入高度图系数；届时以 parity 测试的对照快照做回归门禁。

use std::collections::{HashMap, HashSet};

use hokusai::tile::{empty_tile, TilePixels, TILE_SIZE};
use hokusai::TiledSurface;

/// 单个 tile：像素数据 + 同步/淘汰元数据。
pub struct Tile {
    pixels: Box<TilePixels>,
    /// 内容版本号：每次「写意图」借出（`tile_request_start`）时递增。
    /// 保守策略——借出即视为可能脏，宁可多传不可漏传。
    revision: u64,
    /// 最近访问的全局时钟值（LRU 依据）。
    last_touched: u64,
}

impl Tile {
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
}

impl TiledSurface for PigmentSurface {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.clock += 1;
        let clock = self.clock;
        if self.in_atomic {
            self.dirty.insert((tx, ty));
        }
        let tile = self.tiles.entry((tx, ty)).or_insert_with(|| Tile {
            pixels: empty_tile(),
            revision: 0,
            last_touched: 0,
        });
        tile.revision = clock;
        tile.last_touched = clock;
        &mut tile.pixels
    }

    fn tile_request_end(&mut self, _tx: i32, _ty: i32) {}

    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.tile(tx, ty)
    }

    fn begin_atomic(&mut self) {
        self.in_atomic = true;
        self.dirty.clear();
    }

    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.in_atomic = false;
        self.dirty.drain().collect()
    }

    // draw_dab / get_color / get_color_pigment：全部走 hokusai-core 默认实现，
    // 与 MemSurface 保持逐像素一致。
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
