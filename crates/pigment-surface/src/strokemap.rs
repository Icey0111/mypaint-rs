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
            let mut changed = false;
            for y in 0..N {
                for x in 0..N {
                    let pa = a.map(|t| t[y][x]).unwrap_or([0; 4]);
                    let pb = b.map(|t| t[y][x]).unwrap_or([0; 4]);
                    if pa == pb {
                        continue;
                    }
                    changed = true;
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
            if any || changed {
                shape.tiles.insert(c, mask);
            }
        }
        if shape.tiles.is_empty() {
            None
        } else {
            Some(shape)
        }
    }

    /// 这个形状是否覆盖模型坐标 `(x, y)` 的像素
    /// （`lib/strokemap.py` 的 `touches_pixel`）。
    ///
    /// 坐标按 tile 常量向下取整/取余，负数与源码的 `//`/`%` 一致。
    pub fn touches_pixel(&self, x: i32, y: i32) -> bool {
        let n = N as i32;
        let (tx, ty) = (x.div_euclid(n), y.div_euclid(n));
        let (px, py) = (x.rem_euclid(n) as usize, y.rem_euclid(n) as usize);
        self.tiles
            .get(&(tx, ty))
            .map(|m| m[py * N + px] != 0)
            .unwrap_or(false)
    }

    /// 按像素平移整张形状（`StrokeShape.translate`，`lib/strokemap.py:212`）。
    ///
    /// 源码用 `calc_translation_slices` 把 tile 切成条带再拼到相邻 tile；这里
    /// 逐像素换算到绝对坐标再写回，结果等价（含负方向、跨 tile、以及平移后
    /// 变空的 tile 被丢弃）。用于 ORA 载入时图层原点不是 64 倍数的情况。
    pub fn translate(&mut self, dx: i32, dy: i32) {
        if dx == 0 && dy == 0 {
            return;
        }
        let n = N as i64;
        let mut out: BTreeMap<(i32, i32), Vec<u8>> = BTreeMap::new();
        for ((tx, ty), mask) in &self.tiles {
            for y in 0..N {
                for x in 0..N {
                    if mask[y * N + x] == 0 {
                        continue;
                    }
                    let ax = *tx as i64 * n + x as i64 + dx as i64;
                    let ay = *ty as i64 * n + y as i64 + dy as i64;
                    let ntx = ax.div_euclid(n) as i32;
                    let nty = ay.div_euclid(n) as i32;
                    let px = ax.rem_euclid(n) as usize;
                    let py = ay.rem_euclid(n) as usize;
                    out.entry((ntx, nty))
                        .or_insert_with(|| vec![0u8; N * N])[py * N + px] = 1;
                }
            }
        }
        self.tiles = out;
    }

    /// 单笔的 "v2" 字节串（`save_to_string`）。
    ///
    /// `dx/dy` 与源码同义：tile 坐标的**平移量**（像素）。写 ORA 时调用方传
    /// 图层 bbox 原点的相反数，使落盘的 tile 下标相对图层 PNG（`data.py:1692`
    /// 的 `_write_strokemap(sio, self.strokes, -x, -y)`）。
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
///
/// 三处与源码对齐：
/// - brush 表按**整个文件**去重（源码 `brush2id`），不是只合并相邻同款；
/// - 同一支笔第二次出现时不再写 `b`，`s` 里复用第一次的 id；
/// - 即使没有任何笔迹也写结束标记 `}`（源码无条件 `f.write(b"}")`），
///   所以空图层的 `.dat` 是 1 字节而不是 0 字节。
pub fn write_dat(strokes: &[StrokeShape], dx: i32, dy: i32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut brush_ids: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for shape in strokes {
        if shape.is_empty() {
            continue;
        }
        let id = match brush_ids.get(shape.brush.as_str()) {
            Some(&id) => id,
            None => {
                let id = brush_ids.len() as u32;
                brush_ids.insert(shape.brush.as_str(), id);
                let z = zlib(shape.brush.as_bytes());
                out.push(b'b');
                out.extend_from_slice(&(z.len() as u32).to_be_bytes());
                out.extend_from_slice(&z);
                id
            }
        };
        let bytes = shape.to_bytes(dx, dy);
        out.push(b's');
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
    }
    out.push(b'}');
    out
}

/// 读 `.dat`（`_load_strokemap_from_file`，`data.py:1628-1658`）。
///
/// 源码允许图层原点**不是 64 的倍数**：
/// ```text
/// x = int(translate_x // N) * N;  dx = translate_x % N
/// stroke.init_from_string(tmp, x, y)   # 先按整 tile 平移
/// if (dx, dy) != (0, 0): stroke.translate(dx, dy)  # 再按像素平移
/// ```
/// 这里照做：高位按 tile 平移，低位对 tile 位图做像素平移。
pub fn read_dat(data: &[u8], dx: i32, dy: i32) -> Result<Vec<StrokeShape>, String> {
    let n = N as i32;
    let aligned_x = dx.div_euclid(n) * n;
    let aligned_y = dy.div_euclid(n) * n;
    let rem_x = dx.rem_euclid(n);
    let rem_y = dy.rem_euclid(n);
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
                let mut shape = StrokeShape::from_bytes(&data[i..i + len], aligned_x, aligned_y)?;
                if rem_x != 0 || rem_y != 0 {
                    shape.translate(rem_x, rem_y);
                }
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
    fn eraser_only_diff_keeps_an_empty_shape_like_source() {
        // 源码 `_Tile.new_from_diff` 只要 tile 变了就插入一块（即使掩码全 0），
        // `add_stroke_shape` 因此会记录一条空形状；擦除笔画的 alpha 递减
        // 不会点亮任何阈值位，但 tile 变化本身仍要留痕。
        let mut s = PigmentSurface::new();
        let red = vec![255u8, 0, 0, 255].repeat(64 * 64);
        s.load_from_rgba8(&red, 0, 0, 64, 64);
        let before = s.save_snapshot();
        s.replace_from_rgba8(&vec![0u8; 64 * 64 * 4], 0, 0, 64, 64);
        let after = s.save_snapshot();
        let shape = StrokeShape::from_snapshots(&before, &after).expect("擦除也要有形状");
        assert!(!shape.is_empty());
        assert!(shape.tiles.contains_key(&(0, 0)));
        assert!(shape.tiles[&(0, 0)].iter().all(|b| *b == 0), "掩码全 0");
        // 写盘后仍保留这块全 0 的 tile（源码不丢弃）
        let dat = write_dat(&[shape], 0, 0);
        let back = read_dat(&dat, 0, 0).unwrap();
        assert_eq!(back.len(), 1);
        assert!(back[0].tiles[&(0, 0)].iter().all(|b| *b == 0));
    }

    #[test]
    fn touches_pixel_handles_negative_coordinates() {
        let mut shape = StrokeShape::default();
        let mut mask = vec![0u8; N * N];
        mask[5 * N + 7] = 1;
        shape.tiles.insert((0, 0), mask.clone());
        assert!(shape.touches_pixel(7, 5));
        assert!(!shape.touches_pixel(8, 5));
        // 负数坐标：-1 落在 tile (-1) 的像素 63，不在 (0,0)
        assert!(!shape.touches_pixel(-1, 5));
        shape.tiles.insert((-1, 0), {
            let mut m = vec![0u8; N * N];
            m[5 * N + 63] = 1;
            m
        });
        assert!(shape.touches_pixel(-1, 5));
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

    /// 一个只有单像素的 tile，方便断言平移结果。
    fn dot(tx: i32, ty: i32, x: usize, y: usize, brush: &str) -> StrokeShape {
        let mut mask = vec![0u8; N * N];
        mask[y * N + x] = 1;
        let mut s = StrokeShape { tiles: BTreeMap::new(), brush: brush.to_string() };
        s.tiles.insert((tx, ty), mask);
        s
    }

    /// ORA 图层原点可以不是 64 的倍数（`data.py:1636-1658`）：
    /// 高位按 tile 平移，低位按像素平移。
    #[test]
    fn unaligned_offset_translates_the_bitmap() {
        let dat = write_dat(&[dot(0, 0, 10, 10, "b")], 0, 0);
        // +60 px：tile 0 的 x=10 落到 tile 1 的 x=6
        let a = read_dat(&dat, 60, 0).unwrap();
        assert_eq!(a[0].tiles[&(1, 0)][10 * N + 6], 1);
        // -10 px：落到 tile 0 的 x=0
        let b = read_dat(&dat, -10, 0).unwrap();
        assert_eq!(b[0].tiles[&(0, 0)][10 * N + 0], 1);
        // y 同理
        let c = read_dat(&dat, 0, -10).unwrap();
        assert_eq!(c[0].tiles[&(0, 0)][0 * N + 10], 1);
        // 平移后越界到负 tile 也不能丢：x=10-12=-2 → tile -1, px 62
        let d = read_dat(&dat, -12, 0).unwrap();
        assert_eq!(d[0].tiles[&(-1, 0)][10 * N + 62], 1);
        assert!(!d[0].tiles.values().any(|m| m.iter().filter(|&&v| v == 1).count() != 1));
    }

    /// 写 ORA 时 tile 下标相对图层 bbox：传 `-x,-y` 后能按 `+x,+y` 读回。
    #[test]
    fn relative_tile_origin_round_trips() {
        let shape = dot(3, 2, 5, 7, "b");
        let dat = write_dat(&[shape.clone()], -192, -128); // bbox 原点 (192,128)
        // 'b' + u32 长度 + zlib 笔刷 + 's' + u32 brush_id + u32 长度，之后才是 tile 坐标
        let b_len = u32::from_be_bytes(dat[1..5].try_into().unwrap()) as usize;
        let s_off = 1 + 4 + b_len;
        assert_eq!(dat[s_off], b's');
        let tx_off = s_off + 1 + 8;
        let first_tx = i32::from_be_bytes(dat[tx_off..tx_off + 4].try_into().unwrap());
        let first_ty = i32::from_be_bytes(dat[tx_off + 4..tx_off + 8].try_into().unwrap());
        assert_eq!((first_tx, first_ty), (0, 0), "落盘坐标应相对图层原点");
        let back = read_dat(&dat, 192, 128).unwrap();
        assert_eq!(back[0].tiles, shape.tiles, "按图层原点读回应还原绝对坐标");
    }

    /// 真实 MyPaint（`lib/layer/data.py:_write_strokemap`，本机 MyPaint 2.0
    /// 的 Python + mypaintlib）写出的 `.dat`：读取结果与写回字节都要一致。
    ///
    /// fixture 生成：`.workbuddy/tools/gen-strokemap-fixture.py`；三笔
    /// A/B/A 用来验证 brush 表全文件去重（Python 的 `brush2id`）。
    #[test]
    fn matches_real_mypaint_dat_bytes() {
        let py: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/strokemap_py.dat"
        ));
        let shapes = read_dat(py, 192, 128).unwrap();
        assert_eq!(shapes.len(), 3);
        assert_eq!(
            shapes.iter().map(|s| s.brush.as_str()).collect::<Vec<_>>(),
            ["A", "B", "A"]
        );
        assert_eq!(shapes[0].tiles.keys().copied().collect::<Vec<_>>(), [(3, 2)]);
        assert_eq!(shapes[0].tiles[&(3, 2)][7 * N + 5], 1);

        let ours = write_dat(
            &[dot(3, 2, 5, 7, "A"), dot(3, 2, 9, 9, "B"), dot(4, 2, 1, 1, "A")],
            -192,
            -128,
        );
        // zip 内的 zlib 流不必逐字节相同（Python 用 zlib，Rust 用 miniz_oxide，
        // 同为合法 zlib 且解压结果一致）；比较**解出的结构**。
        let ours_shapes = read_dat(&ours, 192, 128).unwrap();
        assert_eq!(ours_shapes.len(), shapes.len());
        for (a, b) in ours_shapes.iter().zip(shapes.iter()) {
            assert_eq!(a.brush, b.brush);
            assert_eq!(a.tiles, b.tiles);
        }
        assert_eq!(*ours.last().unwrap(), b'}', "结束标记");
        assert_eq!(ours.iter().filter(|&&b| b == b'b').count(), 2, "brush 表去重");
    }

    /// brush 表按全文件去重（源码 `brush2id`），空笔迹也以 `}` 结束。
    #[test]
    fn brush_table_dedup_and_empty_terminator() {
        let dat = write_dat(
            &[dot(0, 0, 1, 1, "A"), dot(0, 0, 2, 2, "B"), dot(0, 0, 3, 3, "A")],
            0,
            0,
        );
        assert_eq!(dat.iter().filter(|&&b| b == b'b').count(), 2, "A/B 各写一次");
        let back = read_dat(&dat, 0, 0).unwrap();
        assert_eq!(
            back.iter().map(|s| s.brush.as_str()).collect::<Vec<_>>(),
            ["A", "B", "A"],
            "第三笔复用 A 的 id"
        );
        assert_eq!(write_dat(&[], 0, 0), b"}", "空笔迹也要写结束符（源码无条件写）");
    }
}
