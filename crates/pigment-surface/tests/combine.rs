//! B2-1 硬门禁：22 种混合/合成模式 × dst_has_alpha × 5 档不透明度，
//! 与直接编译参考源（_mypaint_ref/lib/{blending,compositing,fix15}.hpp）得到的
//! 金标准逐位比对。
//!
//! fixture 由 `.workbuddy/tools/gen-combine-vectors.cpp` 生成（数学 100% 来自
//! 参考头文件，生成器只做调度与 I/O），220 个 case，提交进仓库。
//! 重新生成：`bash E:/MyPaint/.workbuddy/tools/gen-combine-vectors.sh`。

use pigment_surface::combine::{tile_combine, CombineMode};

/// fixture 里 tile 是 8×8（生成器构造）；混合数学逐像素与尺寸无关。
const FIX_TILE: usize = 8 * 8 * 4;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/combine_vectors.bin");

fn read_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn read_f32(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

fn tile_bytes(buf: &[u8], off: usize) -> Vec<u16> {
    (0..FIX_TILE)
        .map(|k| read_u16(buf, off + k * 2))
        .collect()
}

#[test]
fn tile_combine_matches_reference_bit_for_bit() {
    let data = std::fs::read(FIXTURE).expect("fixture 应随仓库提交");
    assert_eq!(&data[0..4], b"PCV1", "fixture 魔数");
    let count = read_u32(&data, 4) as usize;
    assert_eq!(count, 220, "22 模式 × 2 dst_has_alpha × 5 档不透明度");

    let mut off = 8usize;
    let mut checked = 0usize;
    while off < data.len() {
        let mode_i = data[off];
        let dst_has_alpha = data[off + 1] != 0;
        let opacity = read_f32(&data, off + 4);
        let src = tile_bytes(&data, off + 8);
        let dst_in = tile_bytes(&data, off + 8 + FIX_TILE * 2);
        let expected = tile_bytes(&data, off + 8 + FIX_TILE * 4);
        off += 8 + FIX_TILE * 6;
        checked += 1;

        let mode = CombineMode::from_index(mode_i).expect("合法模式索引");
        let mut dst = dst_in.clone();
        tile_combine(mode, &src, &mut dst, dst_has_alpha, opacity);

        let mut diffs = 0usize;
        let mut first_diff = None;
        for k in 0..FIX_TILE {
            if dst[k] != expected[k] {
                diffs += 1;
                if first_diff.is_none() {
                    first_diff = Some((k / 4, k % 4, dst[k], expected[k]));
                }
            }
        }
        assert_eq!(
            diffs,
            0,
            "模式 {:?} dst_has_alpha={} opacity={}: {} 通道不一致，首个 {:?}",
            mode,
            dst_has_alpha,
            opacity,
            diffs,
            first_diff,
        );
    }
    assert_eq!(checked, count);
    assert_eq!(off, data.len(), "fixture 应无尾部数据");
}

/// 元数据与 pixops.cpp/`combine_mode_get_info` 一致（也覆盖 lib/modes.py 的
/// 三个派生集合）。
#[test]
fn mode_metadata_matches_reference() {
    // pixops.cpp:784-811 LUT 的规范名
    let names = [
        "svg:src-over", "svg:multiply", "svg:screen", "svg:overlay", "svg:darken",
        "svg:lighten", "svg:hard-light", "svg:soft-light", "svg:color-burn",
        "svg:color-dodge", "svg:difference", "svg:exclusion", "svg:hue",
        "svg:saturation", "svg:color", "svg:luminosity", "svg:plus", "svg:dst-in",
        "svg:dst-out", "svg:src-atop", "svg:dst-atop", "mypaint:spectral-wgm",
    ];
    for (i, &name) in names.iter().enumerate() {
        let m = CombineMode::from_index(i as u8).unwrap();
        assert_eq!(m.ora_name(), name);
        assert_eq!(CombineMode::from_ora_name(name), Some(m));
    }
    assert_eq!(CombineMode::ALL.len(), 22);

    // compositing.hpp 各 CompositeFunc 的静态标志
    let zahe: Vec<_> = CombineMode::ALL
        .iter()
        .filter(|m| m.zero_alpha_has_effect())
        .collect();
    assert_eq!(zahe, [&CombineMode::DestinationIn, &CombineMode::DestinationAtop]);
    let cda: Vec<_> = CombineMode::ALL
        .iter()
        .filter(|m| m.can_decrease_alpha())
        .collect();
    assert_eq!(
        cda,
        [
            &CombineMode::DestinationIn,
            &CombineMode::DestinationOut,
            &CombineMode::DestinationAtop
        ]
    );
    let zacb: Vec<_> = CombineMode::ALL
        .iter()
        .filter(|m| m.zero_alpha_clears_backdrop())
        .collect();
    assert_eq!(zacb, [&CombineMode::DestinationIn, &CombineMode::DestinationAtop]);

    // lib/modes.py:28 默认模式
    assert_eq!(pigment_surface::combine::DEFAULT_MODE, CombineMode::SpectralWGM);
    assert_eq!(pigment_surface::combine::PASS_THROUGH_MODE, -1);
}
