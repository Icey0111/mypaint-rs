//! 笔画形状图（strokemap）—— `lib/strokemap.py` 的 Rust 版。
//!
//! 每个 [`StrokeShape`] 保存**一笔**改动的 1-bit tile 位图：像素值为 1 表示
//! 这一笔在这个像素上产生了"可感知变化"。格式与 MyPaint 的 "v2" 一致：
//!
//! ```text
//! 逐 tile:  >iiI   tx, ty, 压缩后字节数
//!           zlib(64*64 个 0/1)
//! ```
//!
//! 整张图（`.dat`）再把多笔串起来：
//!
//! ```text
//! 'b' u32(len) zlib(brush JSON)   切换笔刷时写一次
//! 's' u32(brush_id) u32(len) shape_bytes
//! '}'                              结束
//! ```
//!
//! 与源码的差异：本实现不做后台 idle 压缩（`_TileDiffUpdateTask`），
//! 记录时直接算完；`.dat` 的 tile 顺序按 BTreeMap，源码是 dict 序，
//! 读取方不依赖顺序。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use hokusai::tile::TILE_SIZE;

use crate::SurfaceSnapshot;

const N: usize = TILE_SIZE;

/// 一笔的形状：tile 坐标 → 4096 个 0/1。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StrokeShape {
    pub tiles: BTreeMap<(i32, i32), Vec<u8>>,
    /// 画这一笔的笔刷设置（`.myb` JSON，源码 `brush_string`）。
    pub brush: String,
}

impl std::fmt::Debug for StrokeShape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrokeShape")
            .field("tiles", &self.tiles.len())
            .field("brush", &self.brush.len())
            .finish()
    }
}

impl StrokeShape {
    /// 由前后快照求差（`StrokeShape.new_from_snapshots` + `_Tile.new_from_diff`）。
    ///
    /// 阈值照 `pixops.cpp:657-710`：颜色变化 > max(alpha)/16，或 alpha 增加 >
    /// 1/4，或相对 alpha 增加 > 1/64 且 > 旧 alpha/2。
    pub fn from_snapshots(before: &SurfaceSnapshot, after: &SurfaceSnapshot) -> Option<Self> {
        let mut coords: BTreeSet<(i32, i32)> = BTreeSet::new();
        coords.extend(before.coords());
        coords.extend(after.coords());
        let mut shape = StrokeShape::default();
        let half = (1u32 << 15) as i32;
        for c in coords {
            let a = before.tile(c.0, c.1);
            let b = after.tile(c.0, c.1);
            let mut mask = vec![0u8; N * N];
            let mut any = false;
            for y in 0..N {
                for x in 0..N {
                    let pa = a.map(|t| t[y][x]).unwrap_or([0; 4]);
                    let pb = b.map(|t| t[y][x]).unwrap_or([0; 4]);
                    if pa == pb {
                        continue;
                    }
                    let aa = pa[3] as i32;
                    let ab = pb[3] as i32;
                    let mut color_change = 0i32;
                    for i in 0..3 {
                        let ac = (pa[i] as i32 * ab) / half;
                        let bc = (pb[i] as i32 * aa) / half;
                        color_change += (bc - ac).abs();
                    }
                    let max_a = aa.max(ab);
                    let color_ch = color_change > max_a / 16;
                    let alpha_inc = (ab - aa) > half / 4;
                    let rel_inc = (ab - aa) > half / 64 && (ab - aa) > aa / 2;
                    if color_ch || alpha_inc || rel_inc {
                        mask[y * N + x] = 1;
                        any = true;
                    }
                }
            }
            if any {
                shape.tiles.insert(c, mask);
            }
        }
        if shape.tiles.is_empty() {
            None
        } else {
            Some(shape)
        }
    }

    /// 单笔的 "v2" 字节串（`save_to_string`）。
    pub fn to_bytes(&self, dx: i32, dy: i32) -> Vec<u8> {
        debug_assert_eq!(dx % N as i32, 0);
        debug_assert_eq!(dy % N as i32, 0);
        let ox = dx / N as i32;
        let oy = dy / N as i32;
        let mut out = Vec::new();
        for ((tx, ty), mask) in &self.tiles {
            let z = zlib(mask);
            out.extend_from_slice(&(tx + ox).to_be_bytes());
            out.extend_from_slice(&(ty + oy).to_be_bytes());
            out.extend_from_slice(&(z.len() as u32).to_be_bytes());
            out.extend_from_slice(&z);
        }
        out
    }

    /// 从 "v2" 字节串恢复（`init_from_string`）。
    pub fn from_bytes(data: &[u8], dx: i32, dy: i32) -> Result<Self, String> {
        if dx % N as i32 != 0 || dy % N as i32 != 0 {
            return Err("strokemap 平移量必须是 tile 的整数倍".to_string());
        }
        let ox = dx / N as i32;
        let oy = dy / N as i32;
        let mut tiles = BTreeMap::new();
        let mut i = 0usize;
        while i < data.len() {
            if i + 12 > data.len() {
                return Err("strokemap 截断（tile 头）".to_string());
            }
            let tx = i32::from_be_bytes(data[i..i + 4].try_into().unwrap()) + ox;
            let ty = i32::from_be_bytes(data[i + 4..i + 8].try_into().unwrap()) + oy;
            let len = u32::from_be_bytes(data[i + 8..i + 12].try_into().unwrap()) as usize;
            i += 12;
            if i + len > data.len() {
                return Err("strokemap 截断（tile 数据）".to_string());
            }
            let mask = unzlib(&data[i..i + len])?;
            if mask.len() != N * N {
                return Err(format!("strokemap tile 长度 {} != {}", mask.len(), N * N));
            }
            tiles.insert((tx, ty), mask);
            i += len;
        }
        Ok(Self { tiles, brush: String::new() })
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// 多笔 `.dat` 的序列化（`lib/layer/data.py:1782-1808` 的 `_write_strokemap`）。
pub fn write_dat(strokes: &[StrokeShape], dx: i32, dy: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut brush_id = 0u32;
    let mut current: Option<&str> = None;
    for shape in strokes {
        if shape.is_empty() {
            continue;
        }
        if current != Some(shape.brush.as_str()) {
            let z = zlib(shape.brush.as_bytes());
            out.push(b'b');
            out.extend_from_slice(&(z.len() as u32).to_be_bytes());
            out.extend_from_slice(&z);
            current = Some(shape.brush.as_str());
            brush_id += 1;
        }
        let bytes = shape.to_bytes(dx, dy);
        out.push(b's');
        out.extend_from_slice(&(brush_id - 1).to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
    }
    if !out.is_empty() {
        out.push(b'}');
    }
    out
}

/// 读 `.dat`（`_load_strokemap_from_file`）。
pub fn read_dat(data: &[u8], dx: i32, dy: i32) -> Result<Vec<StrokeShape>, String> {
    let mut brushes: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        match data[i] {
            b'b' => {
                i += 1;
                if i + 4 > data.len() {
                    return Err("strokemap 截断（b 头）".to_string());
                }
                let len = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
                i += 4;
                if i + len > data.len() {
                    return Err("strokemap 截断（b 数据）".to_string());
                }
                let bytes = unzlib(&data[i..i + len])?;
                brushes.push(String::from_utf8_lossy(&bytes).into_owned());
                i += len;
            }
            b's' => {
                i += 1;
                if i + 8 > data.len() {
                    return Err("strokemap 截断（s 头）".to_string());
                }
                let id = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
                let len = u32::from_be_bytes(data[i + 4..i + 8].try_into().unwrap()) as usize;
                i += 8;
                if i + len > data.len() {
                    return Err("strokemap 截断（s 数据）".to_string());
                }
                let mut shape = StrokeShape::from_bytes(&data[i..i + len], dx, dy)?;
                shape.brush = brushes.get(id).cloned().unwrap_or_default();
                out.push(shape);
                i += len;
            }
            b'}' => break,
            other => return Err(format!("strokemap 非法标记 {other:?}")),
        }
    }
    Ok(out)
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn unzlib(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut d = ZlibDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out)
        .map_err(|e| format!("zlib 解压失败: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PigmentSurface;
    use hokusai::TiledSurface;

    fn painted() -> (SurfaceSnapshot, SurfaceSnapshot) {
        let mut s = PigmentSurface::new();
        let before = s.save_snapshot();
        // 直接写像素：足够造出一笔的 alpha 变化
        let t = s.tile_request_start(0, 0);
        for y in 10..50 {
            for x in 10..50 {
                t[y][x] = [0, 0, 0, 1 << 15];
            }
        }
        s.tile_request_end(0, 0);
        let after = s.save_snapshot();
        (before, after)
    }

    #[test]
    fn diff_and_dat_roundtrip() {
        let (before, after) = painted();
        let shape = StrokeShape::from_snapshots(&before, &after).expect("应有形状");
        assert!(shape.tiles.contains_key(&(0, 0)));
        let dat = write_dat(&[shape.clone()], 0, 0);
        let back = read_dat(&dat, 0, 0).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].tiles, shape.tiles);
        // 平移一个 tile
        let shifted = read_dat(&dat, N as i32, 0).unwrap();
        assert!(shifted[0].tiles.contains_key(&(1, 0)));
    }

    #[test]
    fn no_change_is_none() {
        let s = PigmentSurface::new();
        let a = s.save_snapshot();
        let b = s.save_snapshot();
        assert!(StrokeShape::from_snapshots(&a, &b).is_none());
    }
}
