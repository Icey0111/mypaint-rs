//! M3 验收门禁：官方笔刷包「全部可用」。
//!
//! 两个断言层次：
//!
//! 1. **解析**：196 支官方 `.myb` 全部能被 `hokusai` 解析成 `Brush`，8 个分组
//!    （含空 Favorites）、key 唯一。这是「导入官方笔刷包」这件事的硬门禁。
//! 2. **作画**：每支笔刷在自家 `PigmentSurface` 上跑一条完整笔画，都必须
//!    产生 dab（有脏 tile，无 panic），且必须真正改变像素——196/196 全绿
//!    才算「官方笔刷包全部可用」。
//!
//! 底色刻意做成棋盘格（而非纯色），否则 blur / smudge / 混色类笔刷在纯色上
//! 无论怎么涂都不会改变像素，"可用"就无从谈起。

use hokusai::{BrushState, TiledSurface};
use pigment_engine::brushlib::BrushLibrary;
use pigment_surface::PigmentSurface;

const TILE: i32 = 64;

/// 铺一块棋盘格底色（覆盖 stroke 区域及相邻 tile，供 smudge/blur/橡皮工作）。
fn undercoat(surface: &mut PigmentSurface) {
    for ty in -1..=1 {
        for tx in -1..=1 {
            let t = surface.tile_request_start(tx, ty);
            for y in 0..TILE as usize {
                for x in 0..TILE as usize {
                    let dark = ((x / 8) + (y / 8)) % 2 == 0;
                    let v = if dark { 20_000u16 } else { 9_000u16 };
                    t[y][x] = [v, v, v, 1 << 15];
                }
            }
            surface.tile_request_end(tx, ty);
        }
    }
}

/// 快照底色区域的像素，用于事后统计真正变化的像素数。
fn snapshot(surface: &PigmentSurface) -> Vec<((i32, i32), Box<hokusai::tile::TilePixels>)> {
    let mut out = Vec::new();
    for ty in -1..=1 {
        for tx in -1..=1 {
            if let Some(t) = surface.tile(tx, ty) {
                out.push(((tx, ty), Box::new(*t)));
            }
        }
    }
    out
}

fn count_changed(
    surface: &PigmentSurface,
    before: &[((i32, i32), Box<hokusai::tile::TilePixels>)],
) -> usize {
    let mut changed = 0;
    for (coord, old) in before {
        if let Some(now) = surface.tile(coord.0, coord.1) {
            for y in 0..TILE as usize {
                for x in 0..TILE as usize {
                    if now[y][x] != old[y][x] {
                        changed += 1;
                    }
                }
            }
        } else {
            changed += TILE as usize * TILE as usize;
        }
    }
    changed
}

#[test]
fn official_pack_parses_completely() {
    let lib = BrushLibrary::embedded();
    assert_eq!(lib.len(), 196, "官方包应为 196 支");
    assert!(lib.errors.is_empty(), "内嵌包不该有扫描错误");

    let groups = lib.groups();
    assert_eq!(
        groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        vec![
            "Dieterle",
            "classic",
            "deevad",
            "favorites",
            "ramon",
            "experimental",
            "tanda",
            "kaerhon_v1"
        ],
        "分组顺序应与官方 order.conf 一致（含空 Favorites 组）"
    );
    assert_eq!(groups[1].display, "Classic", "分组展示名取自 order.conf");
    assert!(groups[3].count == 0, "官方 order.conf 的 Favorites 是空组");
    assert_eq!(groups.iter().map(|g| g.count).sum::<usize>(), 196);

    // key 唯一
    let mut keys: Vec<&str> = lib.entries.iter().map(|e| e.key.as_str()).collect();
    keys.sort();
    let n = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), n, "笔刷 key 必须唯一");

    let mut failures = Vec::new();
    for e in &lib.entries {
        if let Err(err) = e.load() {
            failures.push(format!("{}: {err}", e.key));
        }
    }
    assert!(
        failures.is_empty(),
        "以下笔刷解析失败:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_official_brush_draws_on_canvas() {
    let lib = BrushLibrary::embedded();
    let mut inert_dabs = Vec::new(); // 连 dab 都没落下
    let mut no_pixel_change = Vec::new(); // 落了 dab 但像素没变

    for entry in &lib.entries {
        let brush = match entry.load() {
            Ok(b) => b,
            Err(e) => {
                inert_dabs.push(format!("{}: 解析失败 {e}", entry.key));
                continue;
            }
        };
        let mut surface = PigmentSurface::new();
        undercoat(&mut surface);
        let before = snapshot(&surface);
        let clock = surface.clock();

        // 一条横穿底色的正弦笔画，压感从 0.5 渐升到 1.0
        let mut state = BrushState::default();
        let steps = 40;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let x = 6.0 + t * 52.0;
            let y = 32.0 + (t * std::f32::consts::TAU).sin() * 7.0;
            let pressure = 0.5 + 0.5 * t;
            brush.stroke_to(&mut state, &mut surface, x, y, pressure, 0.0, 0.0, 0.01);
        }

        let dirty = surface.changed_since(clock);
        if dirty.is_empty() {
            inert_dabs.push(entry.key.clone());
            continue;
        }
        if count_changed(&surface, &before) == 0 {
            no_pixel_change.push(entry.key.clone());
        }
    }

    println!("--- 官方笔刷包作画统计（共 {} 支）---", lib.len());
    println!("未产生 dab: {}", inert_dabs.len());
    for k in &inert_dabs {
        println!("    ! {k}");
    }
    println!("有 dab 但像素零变化: {}", no_pixel_change.len());
    for k in &no_pixel_change {
        println!("    ~ {k}");
    }

    assert!(
        inert_dabs.is_empty(),
        "以下笔刷完全没落下 dab（作画管线未跑通）:\n{}",
        inert_dabs.join("\n")
    );
    assert!(
        no_pixel_change.is_empty(),
        "以下笔刷落了 dab 但一个像素都没改变，无法称为「可用」:\n{}",
        no_pixel_change.join("\n")
    );
}
