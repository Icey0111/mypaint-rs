//! 填充结果的形态学 / 模糊后处理 —— `lib/morphology.py` +
//! `lib/fill/morphology.cpp` + `lib/fill/blur.cpp` 的移植。
//!
//! # 源码在做什么
//!
//! `floodfill.py:336-342` 在扫描线填充之后、合成之前，按选项跑两步：
//!
//! | 步骤 | 选项 | 源码 | 本文件 |
//! |---|---|---|---|
//! | 膨胀/腐蚀 | `offset`（±1..±64） | `morphology.cpp:278-290` | [`morph`] |
//! | 高斯模糊 | `feather`（0..64） | `blur.cpp:73-112` | [`blur`] |
//!
//! 两者都只在 **alpha tile 字典**上工作（`coord -> uint16[N*N]`），和扫描线
//! 填充的输出同一层数据结构；因此可以完全独立于填充算法测试。
//!
//! # 形态学：圆盘结构元
//!
//! 源码不是方形核，而是一个离散圆盘：第 `dy` 行的半宽
//! `floor(sqrt((r + 0.5)^2 - dy^2))`（`morphology.cpp:22-30` 的
//! `chord_width`），即像素偏移满足 `dx^2 + dy^2 <= (r + 0.5)^2`。
//! 膨胀取邻域最大值、腐蚀取最小值；邻域越出 tile 字典的像素按透明 0 处理
//! （`morphology.py:24-28` 的 `adjacent_tiles` / `_EMPTY_TILE`）。
//!
//! 源码用滑动 LUT 把每行的横向窗口预先折成"行极值"，再对行做纵向折叠；
//! 本移植保留同样的两阶段结构 —— 结果逐像素一致，只是实现用 `VecDeque`
//! 维护滑动窗口极值。
//!
//! # 模糊：可分离高斯
//!
//! 核由 `blur.cpp:17-39` 的 `blur_factors` 生成：`sigma = 0.3r + 0.3`，
//! `prelim_size = 6*ceil(sigma + 1)`，系数 `fix15_one*fac | 3`（`|3` 是源码
//! 防"系数和略小于 1"的偏置，必须照抄）。先横后纵两趟，均用 fix15 乘法、
//! 累加后 `fix15_short_clamp`（`blur.cpp:88-109`）。
//!
//! # 输出 tile 集合
//!
//! - 膨胀：输入 tile 的 8 邻域也可能被"撑"出内容（`morphology.py:101-103`
//!   的 `complement_adjacent`），所以输出坐标 = 输入 ∪ 8 邻域；
//! - 腐蚀：不扩邻域（`morphology.py:102` 只在 `offset > 0` 时补邻域），
//!   输出坐标 = 输入；
//! - 模糊：总是补 8 邻域（`morphology.py:117`），输出坐标 = 输入 ∪ 8 邻域。
//!
//! 任何整块全透明的输出 tile 都**不写入**结果字典（`morphology.cpp:320`
//! 与 `blur.cpp:157` 的 `if not empty`），这样合成阶段会自然地跳过它。

use std::collections::{HashMap, VecDeque};

use hokusai::tile::TILE_SIZE;
use pigment_surface::combine::fix15;

use super::fill::{AlphaTile, OPAQUE, PIXELS};

/// 一块 tile 的边长（`N`）。
const N: i32 = TILE_SIZE as i32;

/// 一个填充阶段的结果 tile 表（`coord -> alpha tile`）。
pub type FilledTiles = HashMap<(i32, i32), AlphaTile>;

/// 单像素 alpha（跨 tile 坐标；缺 tile → 0）。
///
/// `x`/`y` 是**模型像素坐标**（可以为负）。对应源码 `_EMPTY_TILE` 的读取。
fn alpha_at(tiles: &FilledTiles, x: i32, y: i32) -> u16 {
    let tx = x.div_euclid(N);
    let ty = y.div_euclid(N);
    let px = x.rem_euclid(N) as usize;
    let py = y.rem_euclid(N) as usize;
    match tiles.get(&(tx, ty)) {
        Some(AlphaTile::Full) => OPAQUE,
        Some(AlphaTile::Uniform(a)) => *a,
        Some(AlphaTile::Data(data)) => data[py * TILE_SIZE + px],
        None => 0,
    }
}

/// 把一块 tile 的逐像素数据折成最紧凑的表示；全透明返回 `None`。
fn compact(data: Box<[u16; PIXELS]>) -> Option<AlphaTile> {
    let first = data[0];
    if data.iter().all(|v| *v == first) {
        return match first {
            0 => None,
            OPAQUE => Some(AlphaTile::Full),
            a => Some(AlphaTile::Uniform(a)),
        };
    }
    Some(AlphaTile::Data(data))
}

/// 8 邻域（含自身）的坐标，顺序无关（`fill_common.py:74` 的 `adjacent`）。
fn adjacent8(coord: (i32, i32)) -> [(i32, i32); 9] {
    let (tx, ty) = coord;
    [
        (tx - 1, ty - 1),
        (tx, ty - 1),
        (tx + 1, ty - 1),
        (tx - 1, ty),
        (tx, ty),
        (tx + 1, ty),
        (tx - 1, ty + 1),
        (tx, ty + 1),
        (tx + 1, ty + 1),
    ]
}

/// 离散圆盘第 `dy` 行的半宽（`morphology.cpp:22-30`）。
fn chord_half_width(radius: i32, dy: i32) -> i32 {
    let r = radius as f64 + 0.5;
    ((r * r - (dy as f64) * (dy as f64)).sqrt().floor()) as i32
}

/// 把 `(2r+N)^2` 的输入窗口从 tile 字典里取出来（缺 tile 补 0）。
///
/// 行主序，边长 `N + 2r`；输入坐标 `(i, j)` 对应模型像素
/// `(tx*N + i - r, ty*N + j - r)`。
fn gather(tiles: &FilledTiles, coord: (i32, i32), radius: i32) -> Vec<u16> {
    let (tx, ty) = coord;
    let width = (N + 2 * radius) as usize;
    let mut input = vec![0u16; width * width];
    let base_x = tx * N;
    let base_y = ty * N;
    // tile u16 是 16 位 alpha；按模型坐标逐点取样（跨邻域 tile 也正确）。
    for j in 0..width {
        let y = base_y + j as i32 - radius;
        // 同一行的 tile 是固定的，先算好行内 tile 边界，减少 div_euclid
        for i in 0..width {
            let x = base_x + i as i32 - radius;
            input[j * width + i] = alpha_at(tiles, x, y);
        }
    }
    input
}

/// 滑动窗口极值（`max = true` 取最大，否则取最小）。
///
/// `src` 长度 `width`，窗口宽 `w`，输出 `width - w + 1` 个值。
/// 单调队列实现，O(width)。
fn sliding_extreme(src: &[u16], w: usize, max: bool) -> Vec<u16> {
    debug_assert!(w >= 1 && w <= src.len());
    let mut out = Vec::with_capacity(src.len() - w + 1);
    let mut q: VecDeque<usize> = VecDeque::new();
    for i in 0..src.len() {
        while let Some(&back) = q.back() {
            let worse = if max {
                src[back] <= src[i]
            } else {
                src[back] >= src[i]
            };
            if worse {
                q.pop_back();
            } else {
                break;
            }
        }
        q.push_back(i);
        if q.front().is_some_and(|&front| front + w <= i) {
            q.pop_front();
        }
        if i + 1 >= w {
            out.push(src[*q.front().unwrap()]);
        }
    }
    out
}

/// 形态学膨胀/腐蚀（`morphology.py:95-111` + `morphology.cpp:278-290`）。
///
/// `offset > 0` 膨胀、`< 0` 腐蚀，半径是 `abs(offset)`；`0` 原样返回。
/// 输出全透明 tile 会被丢弃（源码的 `morphed` 只收非空 tile）。
pub fn morph(offset: i32, tiles: FilledTiles) -> FilledTiles {
    if offset == 0 || tiles.is_empty() {
        return tiles;
    }
    let radius = offset.abs();
    debug_assert!(radius <= N, "offset 由调用方夹到 ±TILE_SIZE");
    let dilating = offset > 0;

    // 输出坐标：膨胀要覆盖 8 邻域（`complement_adjacent`），腐蚀只处理原 tile。
    let mut coords: Vec<(i32, i32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &coord in tiles.keys() {
        let list = if dilating {
            adjacent8(coord).to_vec()
        } else {
            vec![coord]
        };
        for c in list {
            if seen.insert(c) {
                coords.push(c);
            }
        }
    }

    let width = (N + 2 * radius) as usize;
    let mut result = FilledTiles::new();
    for coord in coords {
        let input = gather(&tiles, coord, radius);
        // 输出 `(x, y)` 的第 c 个结构元行取输入行 `y + c`，窗口宽由
        // `dy = c - r` 决定（`morphology.cpp:186-216`：LUT 随 y 下移，
        // 每个输入行对每种宽度都有横向极值）。这里对每个 (y, c) 现场算
        // 该行的滑动极值，再纵向折叠；省掉 C++ 的 LUT 旋转，结果一致。
        let mut data = Box::new([0u16; PIXELS]);
        for y in 0..N as usize {
            // 先算好本输出行需要的那 2r+1 条横向极值线（每条 N 个值）
            let mut lines: Vec<Vec<u16>> = Vec::with_capacity(2 * radius as usize + 1);
            for c in 0..=(2 * radius as usize) {
                let dy = c as i32 - radius;
                let hw = chord_half_width(radius, dy);
                let w = (2 * hw + 1) as usize;
                let start = (radius - hw) as usize;
                let row = &input[(y + c) * width..(y + c + 1) * width];
                let ext = sliding_extreme(row, w, dilating);
                let mut line = Vec::with_capacity(N as usize);
                for x in 0..N as usize {
                    line.push(ext[x + start]);
                }
                lines.push(line);
            }
            for x in 0..N as usize {
                let mut v = if dilating { 0 } else { OPAQUE };
                for line in &lines {
                    v = if dilating {
                        v.max(line[x])
                    } else {
                        v.min(line[x])
                    };
                }
                data[y * TILE_SIZE + x] = v;
            }
        }
        if let Some(tile) = compact(data) {
            result.insert(coord, tile);
        }
    }
    result
}

/// 高斯系数（`blur.cpp:17-39`），`fix15` 定点。
fn blur_factors(radius: i32) -> Vec<u32> {
    let sigma = 0.3 * radius as f32 + 0.3;
    let prelim_size = 6 * (sigma + 1.0).ceil() as usize;
    let mul = 1.0 / (2.0 * std::f32::consts::PI * sigma * sigma).sqrt();
    let exp_mul = 1.0 / (2.0 * sigma * sigma);
    let center = (prelim_size - 1) / 2;
    (0..prelim_size)
        .map(|i| {
            let d = center as i32 - i as i32;
            let fac = mul * (-(d * d) as f32 * exp_mul).exp();
            ((fix15::ONE as f32 * fac) as u32) | 3
        })
        .collect()
}

/// 高斯模糊（`morphology.py:114-122` + `blur.cpp:73-112`）。
///
/// `radius` 是选项里的 feather；`0` 原样返回。先横后纵、fix15 累加后夹紧。
/// 输出坐标 = 输入 ∪ 8 邻域（`complement_adjacent`），全透明结果丢弃。
pub fn blur(radius: i32, tiles: FilledTiles) -> FilledTiles {
    if radius <= 0 || tiles.is_empty() {
        return tiles;
    }
    let factors = blur_factors(radius);
    let r = ((factors.len() - 1) / 2) as i32;
    debug_assert!(r <= N, "feather 由调用方夹到 [0, TILE_SIZE]");
    let width = (N + 2 * r) as usize;

    let mut coords: Vec<(i32, i32)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &coord in tiles.keys() {
        for c in adjacent8(coord) {
            if seen.insert(c) {
                coords.push(c);
            }
        }
    }

    let mut result = FilledTiles::new();
    for coord in coords {
        let input = gather(&tiles, coord, r);
        // 横向：对每行做 (N+2r) 个窗口卷积，只保留中间 N 个输出。
        // C++ 的 `factors` 长度是偶数，但卷积只用前 `2r+1` 个
        // （`blur.cpp:91-94` 的 `xoffs < r+1`），最后一个系数不参与。
        let used = (2 * r + 1) as usize;
        let mut horizontal = vec![0u16; width * N as usize];
        for y in 0..width {
            let row = &input[y * width..(y + 1) * width];
            for x in 0..N as usize {
                let mut sum: u32 = 0;
                for k in 0..used {
                    sum += fix15::mul(row[x + k] as u32, factors[k]);
                }
                horizontal[y * N as usize + x] = fix15::short_clamp(sum);
            }
        }
        // 纵向：每个 x 列，从 horizontal 的 N+2r 行里取 N 个窗口。
        let mut data = Box::new([0u16; PIXELS]);
        for x in 0..N as usize {
            for y in 0..N as usize {
                let mut sum: u32 = 0;
                for k in 0..used {
                    sum += fix15::mul(horizontal[(y + k) * N as usize + x] as u32, factors[k]);
                }
                data[y * TILE_SIZE + x] = fix15::short_clamp(sum);
            }
        }
        if let Some(tile) = compact(data) {
            result.insert(coord, tile);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一块 `f(x, y) -> alpha` 的 tile。
    fn tile(f: impl Fn(usize, usize) -> u16) -> AlphaTile {
        let mut data = Box::new([0u16; PIXELS]);
        for y in 0..TILE_SIZE {
            for x in 0..TILE_SIZE {
                data[y * TILE_SIZE + x] = f(x, y);
            }
        }
        compact(data).expect("测试 tile 不应全透明")
    }

    fn alpha(tiles: &FilledTiles, x: i32, y: i32) -> u16 {
        alpha_at(tiles, x, y)
    }

    /// 圆盘半径 1：中心一个不透明像素膨胀成 3×3 方块
    /// （`(1.5)^2 = 2.25`，对角 `1+1=2 <= 2.25`）。
    #[test]
    fn dilation_grows_a_single_pixel_into_the_radius_one_disk() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), tile(|x, y| if (x, y) == (10, 10) { OPAQUE } else { 0 }));
        let out = morph(1, tiles);
        for y in 9..=11 {
            for x in 9..=11 {
                assert_eq!(alpha(&out, x, y), OPAQUE, "({x},{y}) 在圆盘内应被撑满");
            }
        }
        assert_eq!(alpha(&out, 8, 10), 0, "圆盘外的邻居仍是透明");
        assert_eq!(alpha(&out, 12, 10), 0);
    }

    /// 半径 2 的圆盘：中心行半宽 2、±1 行半宽 2、±2 行半宽 1
    /// （`floor(sqrt(6.25-4)) = 1`）。
    #[test]
    fn dilation_uses_the_circular_not_square_structuring_element() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), tile(|x, y| if (x, y) == (20, 20) { OPAQUE } else { 0 }));
        let out = morph(2, tiles);
        assert_eq!(alpha(&out, 18, 20), OPAQUE);
        assert_eq!(alpha(&out, 22, 20), OPAQUE);
        assert_eq!(alpha(&out, 19, 18), OPAQUE, "±2 行只到半宽 1");
        assert_eq!(alpha(&out, 21, 22), OPAQUE);
        assert_eq!(alpha(&out, 18, 18), 0, "角上（dx=dy=2）在圆外（8 > 6.25）");
        assert_eq!(alpha(&out, 22, 22), 0);
    }

    /// 腐蚀：一个 3×3 不透明方块，半径 1 后只剩中心像素。
    #[test]
    fn erosion_shrinks_and_drops_out_of_bounds_neighbours() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), tile(|x, y| {
            if (9..=11).contains(&x) && (9..=11).contains(&y) {
                OPAQUE
            } else {
                0
            }
        }));
        let out = morph(-1, tiles);
        assert_eq!(alpha(&out, 10, 10), OPAQUE, "唯一的内部像素应保留");
        assert_eq!(alpha(&out, 9, 10), 0, "边缘被腐蚀");
        assert_eq!(alpha(&out, 10, 9), 0);
        // 腐蚀不扩邻域：结果里只有原 tile
        assert_eq!(out.len(), 1);
    }

    /// 腐蚀遇到"全不透明 tile + 缺邻 tile"时，边界必须按透明 0 处理
    /// （`morphology.py:24-28` 的 `_EMPTY_TILE`），因此边缘一圈会变透明。
    #[test]
    fn erosion_treats_missing_neighbour_tiles_as_transparent() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), AlphaTile::Full);
        let out = morph(-1, tiles);
        assert_eq!(alpha(&out, 0, 0), 0, "tile 边缘缺邻域 → 被腐蚀掉");
        assert_eq!(alpha(&out, 1, 1), OPAQUE, "内部仍不透明");
        assert_eq!(alpha(&out, 63, 63), 0);
    }

    /// 膨胀可以跨 tile：左块右边缘的像素把右块左边缘也撑起来。
    #[test]
    fn dilation_crosses_tile_boundaries() {
        let mut tiles = FilledTiles::new();
        tiles.insert(
            (0, 0),
            tile(|x, y| if x == 63 && y == 32 { OPAQUE } else { 0 }),
        );
        let out = morph(1, tiles);
        assert_eq!(alpha(&out, 64, 32), OPAQUE, "右邻块 x=0 应被撑到");
        assert_eq!(alpha(&out, 65, 32), 0);
        assert!(out.contains_key(&(1, 0)), "右邻块应出现在结果里");
    }

    /// 模糊核的中心系数最大、系数单调不增、总和约等于 1。
    #[test]
    fn blur_factors_are_a_normalized_symmetric_kernel() {
        let f = blur_factors(8);
        assert_eq!(f.len() % 2, 0, "prelim_size 恒为偶数（6*ceil(...)）");
        // 实际参与卷积的是前 2r+1 个（`blur.cpp:91-94`），中心在 index r。
        let r = (f.len() - 1) / 2;
        assert!(f[r] >= f[r - 1], "中心系数应最大");
        assert!(f[0] < f[r]);
        let sum: u32 = f[..2 * r + 1].iter().sum();
        assert!(
            (sum as i32 - fix15::ONE as i32).abs() < 2048,
            "系数和应接近 1（实得 {sum}）"
        );
    }

    /// 模糊把硬边变软：左边一半不透明、右边一半透明 → 交界处出现中间值。
    #[test]
    fn blur_softens_a_hard_edge() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), tile(|x, _| if x < 32 { OPAQUE } else { 0 }));
        let out = blur(8, tiles);
        let edge = alpha(&out, 31, 10);
        assert!(edge > 0 && edge < OPAQUE, "交界内侧应变成半透明（实得 {edge}）");
        // 远离边界的地方仍接近原值（高斯核被截断，系数和略小于 1）
        assert!(
            alpha(&out, 5, 10) > (OPAQUE as u32 * 9 / 10) as u16,
            "远离边界应基本不变（实得 {}）",
            alpha(&out, 5, 10)
        );
        assert!(alpha(&out, 63, 10) < OPAQUE / 4);
    }

    /// 模糊跨 tile：左块右边缘的不透明像素会在右块左边缘留下衰减值。
    #[test]
    fn blur_crosses_tile_boundaries() {
        let mut tiles = FilledTiles::new();
        tiles.insert(
            (0, 0),
            tile(|x, y| if x == 63 && y == 32 { OPAQUE } else { 0 }),
        );
        let out = blur(8, tiles);
        let v = alpha(&out, 64, 32);
        assert!(v > 0 && v < OPAQUE, "右邻块应收到模糊值（实得 {v}）");
        assert!(out.contains_key(&(1, 0)), "右邻块应出现在结果里");
    }

    /// 常量 tile 走 `Full`/`Uniform` 时，模糊后应仍是同一常量
    /// （`blur.cpp:78-80` 的两个常量短路在数值上等价）。
    #[test]
    fn blur_keeps_constant_tiles_constant() {
        let mut tiles = FilledTiles::new();
        tiles.insert((0, 0), AlphaTile::Uniform(12345));
        let out = blur(4, tiles);
        // 中心 tile 大面积为常量；只有跨 tile 边缘因缺邻域才衰减。
        // `| 3` 偏置会让常量值偏大几个 fix15 单位（源码如此）。
        let v = alpha(&out, 32, 32);
        assert!((v as i32 - 12345).abs() <= 16, "常量应基本保持（实得 {v}）");
    }
}
