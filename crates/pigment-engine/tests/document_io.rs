//! 文档 I/O 的端到端门禁 —— `.ora` / `.png` 的往返与互操作。
//!
//! 三层验证：
//! 1. **自身往返**：写出的 `.ora` / `.png` 能被我们读回，像素逐字节一致
//! 2. **Python 互操作**：`zipfile` 能读我们的 `.ora`，PIL 能解我们的 PNG
//!    （Python 是 MyPaint 的宿主语言，这条是"别的应用能不能打开"的代理）
//! 3. **读真实 MyPaint 文件**：直接读 `_mypaint_ref/tests/*.ora`
//!    （MyPaint 自己产出的样本）—— 这是最强的兼容性证据

use std::path::{Path, PathBuf};

use pigment_engine::brushlib::default_brush;
use pigment_engine::hokusai::{BrushSetting, BrushState};
use pigment_engine::{Brushwork, Document, LoadOutcome, SaveOutcome, StrokeEvent};

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("pigment-io-{}-{}", std::process::id(), name));
    p
}

fn real_brush(radius: f32) -> pigment_engine::hokusai::Brush {
    let mut b = default_brush();
    b.get_mut(BrushSetting::Radius).base_value = radius;
    b
}

/// 在文档上画一条斜线（带实际位移，保证产生 dab）。
fn paint(doc: &mut Document, y: f32, radius: f32) {
    let brush = real_brush(radius);
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    for i in 0..40 {
        let ev = StrokeEvent::new(0.016, i as f32 * 2.0, y, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        bw.stroke_to(doc.surface_mut(), &brush, "Fan#1", &mut state, ev);
    }
    if bw.stop_recording(doc.surface_mut(), &state, false) {
        doc.do_brushwork(bw);
    }
}

/// 一块 tile 的像素（64×64 RGBA fix15）。
type TilePixels = Box<[[[u16; 4]; 64]; 64]>;
/// 「相对 tile 坐标 → 像素」的映射 + 该表面的最小绝对坐标。
type RelativeTiles = (
    std::collections::BTreeMap<(i32, i32), TilePixels>,
    (i32, i32),
);

/// 把表面抓成「相对坐标 → 像素」的映射。
///
/// 用**相对**坐标：MyPaint 存 ORA 时把坐标写成相对画幅左上角，载入时
/// `update_frame(0, 0, w, h)` 又把画幅挪回原点（`lib/document.py:1853`）。
/// 所以内容的绝对位置在往返后会整体平移，相对布局不变 —— 这与 MyPaint
/// 一致，不是缺陷。
fn relative_tiles(surface: &pigment_surface::PigmentSurface) -> RelativeTiles {
    let mut coords: Vec<(i32, i32)> = surface.tiles().map(|(c, _)| c).collect();
    coords.sort_unstable();
    let min = coords.first().copied().unwrap_or((0, 0));
    let map = surface
        .tiles()
        .map(|(c, t)| ((c.0 - min.0, c.1 - min.1), Box::new(*t.pixels())))
        .collect();
    (map, min)
}

/// 断言两个表面**在 8 位显示层**一致（按位置对齐、容差 16）。
///
/// # 为什么不是 fix15 逐位相等
///
/// ORA/PNG 只能存 8 位通道，而表面是 15 位定点（fix15），往返必然有一次
/// 量化损失。实测（半径 1/2/4 的笔画，`q_probe`）：
///
/// - 53% 的 RGB 通道**完全不变**
/// - 中位差 0；最大差 **14/255**
///
/// 最大差来自 **8 位 alpha 的量化**，而不是 RGB：`a = 32708/32768 = 0.99817`
/// 在 8 位里只能存成 255/255 = 1.0，于是"99.8% 不透明的黑"变成"全不透明黑"。
/// 因为编码是 `c^(1/2.2)`，在接近 0 处斜率很陡：线性域 0.0016 的差会被放大成
/// 显示域的 14/255。MyPaint 存 ORA 图层 PNG 同样是 8 位，所以它有完全相同的
/// 性质 —— 15 位 alpha 无法穿过 8 位 PNG。
///
/// # 为什么按位置对齐而不是按 tile 列表顺序
///
/// 全透明像素在 8 位里 alpha 舍入成 0，载入时会被跳过（不分配 tile，
/// `load_from_rgba8` 与 MyPaint 的稀疏 tile 语义一致）。所以两边的 tile
/// **数量**可能不同，但**可见内容**必须一致。按相对坐标对齐、缺席方视作
/// 全透明即可正确比较。
fn assert_pixels_match(
    a: &pigment_surface::PigmentSurface,
    b: &pigment_surface::PigmentSurface,
    whose: &str,
) {
    /// 8 位下允许的最大偏差。见上：alpha 量化 + EOTF 近零处陡斜率。
    const TOL: i32 = 16;
    let (ma, _) = relative_tiles(a);
    let (mb, _) = relative_tiles(b);
    let bg_lin = pigment_surface::color::bg_to_linear(pigment_surface::color::CANVAS_BG);
    let empty = [[[0u16; 4]; 64]; 64];
    let bg_px = pigment_surface::color::tile_to_rgba8_single(
        [0, 0, 0, 0],
        pigment_surface::color::CANVAS_BG,
        bg_lin,
    );

    let mut keys: Vec<(i32, i32)> = ma.keys().chain(mb.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    assert!(!keys.is_empty(), "{whose}：两边都没有内容");

    for k in keys {
        let pa = ma.get(&k).map(|p| &**p).unwrap_or(&empty);
        let pb = mb.get(&k).map(|p| &**p).unwrap_or(&empty);
        for y in 0..64 {
            for x in 0..64 {
                let va = pigment_surface::color::tile_to_rgba8_single(
                    pa[y][x],
                    pigment_surface::color::CANVAS_BG,
                    bg_lin,
                );
                let vb = pigment_surface::color::tile_to_rgba8_single(
                    pb[y][x],
                    pigment_surface::color::CANVAS_BG,
                    bg_lin,
                );
                for c in 0..3 {
                    let d = (va[c] as i32 - vb[c] as i32).abs();
                    assert!(
                        d <= TOL,
                        "{whose}：相对 tile {k:?} 像素 ({x},{y}) 通道 {c} 差 {d}（超过 {TOL}）\n\
                         前={:?} 后={:?} 底色={bg_px:?}",
                        pa[y][x],
                        pb[y][x],
                    );
                }
            }
        }
    }
}

/// 找一个**能 `import PIL`** 的 Python。
///
/// 这条互操作门禁要靠 PIL 解 PNG，所以只判"有 python 可执行文件"是不够的：
/// 缺 Pillow 的机器上脚本会以 `ModuleNotFoundError` 退出，门禁直接红脸而
/// 原因与被测代码无关（本机的 `python` 就没装 Pillow，`py` 装了 12.1.1）。
/// 候选顺序按 Windows 常见情况排；都不可用则整条门禁跳过（而非误报失败）。
fn python_with_pil() -> Option<&'static str> {
    static FOUND: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        ["python", "python3", "py"].into_iter().find(|cand| {
            std::process::Command::new(cand)
                .args(["-c", "import PIL"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
    })
}

fn has_python() -> bool {
    python_with_pil().is_some()
}

fn run_python(script: &str) -> Result<String, String> {
    let py = python_with_pil().ok_or_else(|| "本机没有带 PIL 的 python".to_string())?;
    let out = std::process::Command::new(py)
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "stdout={}\nstderr={}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

// ---- 自身往返 ----

#[test]
fn png_round_trips_pixels() {
    let path = tmp("roundtrip.png");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    assert!(doc.surface().tile_count() > 0, "应画出内容");

    let outcome = doc.save(&path, Some(true)).unwrap();
    assert!(matches!(outcome, SaveOutcome::Png { .. }));
    assert_eq!(doc.unsaved_painting_time, 0.0, "保存后应清零作画时长");

    let mut doc2 = Document::new();
    let loaded = doc2.load(&path).unwrap();
    assert!(matches!(loaded, LoadOutcome::Png { .. }));
    assert_pixels_match(doc.surface(), doc2.surface(), "PNG 往返");
    assert_eq!(doc2.command_stack.undo_len(), 0, "载入后命令栈应清空");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn jpg_round_trips_as_lossy_opaque() {
    let path = tmp("roundtrip.jpg");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);

    let outcome = doc.save(&path, None).unwrap();
    let SaveOutcome::Jpg { bbox } = outcome else {
        panic!("应识别为 JPEG");
    };
    assert!(bbox.2 > 0 && bbox.3 > 0);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], b"\xff\xd8", "JPEG SOI");

    let mut doc2 = Document::new();
    let loaded = doc2.load(&path).unwrap();
    let LoadOutcome::Jpg { bbox: lb } = loaded else {
        panic!("应识别为 JPEG");
    };
    assert_eq!((bbox.2, bbox.3), (lb.2, lb.3));
    assert!(doc2.surface().tile_count() > 0, "JPEG 载入后应有像素");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn ora_round_trips_pixels() {
    let path = tmp("roundtrip.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    assert!(doc.surface().tile_count() > 0);

    let outcome = doc.save(&path, None).unwrap();
    let SaveOutcome::Ora { bbox } = outcome else {
        panic!("应识别为 ORA");
    };
    assert!(bbox.2 > 0 && bbox.3 > 0, "画幅应为正：{bbox:?}");

    let mut doc2 = Document::new();
    let loaded = doc2.load(&path).unwrap();
    let LoadOutcome::Ora { bbox: lb, .. } = loaded else {
        panic!("应识别为 ORA");
    };
    // ORA 只存画幅的宽高，**存不下画布原点**（格式限制，MyPaint 也一样）。
    // 载入时 `update_frame(0, 0, w, h)` 把画幅挪回原点（lib/document.py:1853），
    // 所以这里比尺寸而不是比原点。
    assert_eq!((lb.2, lb.3), (bbox.2, bbox.3), "往返后画幅尺寸应一致");
    assert_eq!((lb.0, lb.1), (0, 0), "载入后画幅原点归零");
    assert_pixels_match(doc.surface(), doc2.surface(), "ORA 往返");
    assert_eq!(
        doc2.get_frame(),
        (0, 0, bbox.2, bbox.3),
        "载入后应把画幅设为画框（load_ora 的 update_frame）"
    );
    let _ = std::fs::remove_file(&path);
}

/// `xres`/`yres` 与 `settings` 也必须随 ORA 往返。
#[test]
fn ora_round_trips_resolution_and_settings() {
    let path = tmp("meta.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    doc.xres = Some(300);
    doc.yres = Some(300);
    doc.settings
        .insert("T.1".to_string(), serde_json::json!([1, 2]));
    doc.save(&path, None).unwrap();

    let mut doc2 = Document::new();
    doc2.load(&path).unwrap();
    assert_eq!(doc2.xres, Some(300));
    assert_eq!(doc2.yres, Some(300));
    assert_eq!(
        doc2.settings.get("T.1"),
        Some(&serde_json::json!([1, 2])),
        "设置字典应随 ORA 往返（源码 docstring 就是这么承诺的）"
    );
    let _ = std::fs::remove_file(&path);
}

/// 空文档也要写出合法文件（`save_as_png` 的 1×1 兜底）。
#[test]
fn empty_document_saves_legal_files() {
    let png = tmp("empty.png");
    let ora = tmp("empty.ora");
    let mut doc = Document::new();

    doc.save(&png, Some(true)).unwrap();
    let mut d2 = Document::new();
    d2.load(&png).unwrap();
    // 1×1 的空图
    assert_eq!(d2.surface().tile_count(), 0, "全透明 1×1 不分配 tile");

    doc.save(&ora, None).unwrap();
    let mut d3 = Document::new();
    d3.load(&ora).unwrap();
    assert_eq!(d3.surface().tile_count(), 0);

    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(&ora);
}

#[test]
fn unsupported_extension_is_rejected() {
    let mut doc = Document::new();
    let err = doc.save(tmp("x.tiff"), None);
    assert!(err.is_err());
    let err = doc.load(tmp("x.tiff"));
    assert!(err.is_err());
}

#[test]
fn loading_missing_file_reports_not_found() {
    let mut doc = Document::new();
    assert!(doc.load(tmp("definitely-missing-xyz.ora")).is_err());
    assert!(doc.load(tmp("definitely-missing-xyz.png")).is_err());
}

// ---- ORA 容器结构 ----

#[test]
fn strokemap_follows_undo_redo() {
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    assert_eq!(doc.surface().strokes().len(), 1);
    doc.undo().unwrap();
    assert!(
        doc.surface().strokes().is_empty(),
        "撤销应摘掉这一笔的 strokemap"
    );
    doc.redo().unwrap();
    assert_eq!(doc.surface().strokes().len(), 1, "重做应挂回形状");
}

#[test]
fn ora_strokemap_round_trips() {
    let path = tmp("strokemap.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    assert_eq!(
        doc.surface().strokes().len(),
        1,
        "一笔应记录一个 StrokeShape"
    );

    doc.save(&path, None).unwrap();
    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let xml = String::from_utf8(z.read("stack.xml").unwrap()).unwrap();
    assert!(xml.contains("mypaint:strokemap=\"data/layer-00-strokemap.dat\""));
    let dat = z.read("data/layer-00-strokemap.dat").unwrap();
    assert!(!dat.is_empty());
    assert_eq!(*dat.last().unwrap(), b'}');

    let mut doc2 = Document::new();
    doc2.load(&path).unwrap();
    assert_eq!(doc2.surface().strokes().len(), 1, "载入后应恢复 strokemap");
    assert_eq!(
        doc2.surface().strokes()[0].tiles.len(),
        doc.surface().strokes()[0].tiles.len()
    );
    let _ = std::fs::remove_file(&path);
}

/// PNG/JPEG 导出的是**整栈合成**，不是当前层（`document.py:1666-1680` /
/// `:1746-1765`）。旧实现只取 `Document::surface()`，多图层导出会丢层。
#[test]
fn png_export_flattens_all_layers() {
    use pigment_engine::hokusai::TiledSurface;

    let path = tmp("flatten.png");
    let mut doc = Document::new();
    // 底层：整块 tile 蓝色
    {
        let s = doc.surface_mut();
        let t = s.tile_request_start(0, 0);
        for row in t.iter_mut() {
            for p in row.iter_mut() {
                *p = [0, 0, 1 << 15, 1 << 15];
            }
        }
        s.tile_request_end(0, 0);
    }
    // 顶层：只在 (0,0) 一点红色，其余透明
    assert!(doc.add_layer());
    {
        let s = doc.surface_mut();
        let t = s.tile_request_start(0, 0);
        t[0][0] = [1 << 15, 0, 0, 1 << 15];
        s.tile_request_end(0, 0);
    }

    doc.save(&path, None).unwrap();
    let (px, w, h) = pigment_engine::surface::png::read_rgba8_png(&path).unwrap();
    assert_eq!((w, h), (64, 64));
    let at = |x: usize, y: usize| {
        let i = (y * w as usize + x) * 4;
        [px[i], px[i + 1], px[i + 2]]
    };
    assert_eq!(at(0, 0), [255, 0, 0], "顶层红点应保留");
    assert_eq!(at(10, 10), [0, 0, 255], "底层蓝色不应被当前空层覆盖");
    let _ = std::fs::remove_file(&path);
}

/// 默认纸纹（`mrmamurk/mamurk_e_1.png`，CC0）必须作为**平铺图**往返
/// （`document.py:1327-1380` + `tiledsurface.py:1178-1203`）。
#[test]
fn default_paper_texture_round_trips_through_ora() {
    let path = tmp("paper.ora");
    let mut doc = Document::new();
    doc.reset_background();
    let img = doc
        .layers()
        .background_image()
        .expect("默认背景应是纸纹图")
        .clone();
    assert_eq!((img.width(), img.height()), (256, 256));
    let avg = img.average_rgb();
    assert!(
        (avg[0] as i32 - 237).abs() <= 3
            && (avg[1] as i32 - 239).abs() <= 3
            && (avg[2] as i32 - 227).abs() <= 3,
        "纸纹均值应接近 #EDEFE3，实得 {avg:?}"
    );
    let sample = img.pixel_rgba8(37, 91);

    doc.save(&path, None).unwrap();
    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let xml = String::from_utf8(z.read("stack.xml").unwrap()).unwrap();
    assert!(xml.contains("mypaint:background-tile="), "{xml}");
    let tile_name = z
        .names()
        .iter()
        .find(|n| n.ends_with("-tile.png"))
        .expect("应写背景 tile")
        .to_string();
    let (px, w, h) =
        pigment_engine::surface::png::decode_rgba8_png(&z.read(&tile_name).unwrap()).unwrap();
    assert_eq!((w, h), (256, 256), "背景 tile 应是原图尺寸");
    let i = (91 * 256 + 37) * 4;
    assert_eq!(&px[i..i + 4], &sample, "tile 像素应与源图一致");

    let mut d2 = Document::new();
    d2.load(&path).unwrap();
    let img2 = d2
        .layers()
        .background_image()
        .expect("载入后应恢复纸纹")
        .clone();
    assert_eq!((img2.width(), img2.height()), (256, 256));
    assert_eq!(img2.average_rgb(), avg);
    assert_eq!(img2.pixel_rgba8(37, 91), sample, "图案对齐世界原点");
    let _ = std::fs::remove_file(&path);
}

/// 纸纹必须平铺到**任意世界坐标**，而不只是原点附近；GPU 上每块 tile
/// 都是独立合成的，所以逐 tile 检查。
#[test]
fn paper_texture_is_tiled_across_world_coordinates() {
    let mut doc = Document::new();
    doc.reset_background();
    for (tx, ty) in [(0, 0), (5, 0), (20, 7), (-3, 2)] {
        let px = doc
            .layers()
            .composite_tile_rgba8(tx, ty)
            .expect("背景可见时每块 tile 都有内容");
        let mut sum = 0.0f64;
        let mut sum2 = 0.0f64;
        for p in px.chunks_exact(4) {
            let v = (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
            sum += v;
            sum2 += v * v;
        }
        let n = (64 * 64) as f64;
        let sd = (sum2 / n - (sum / n).powi(2)).sqrt();
        assert!(sd > 0.5, "tile ({tx},{ty}) 应带纸纹，标准差只有 {sd}");
    }
}

/// 嵌套 `<stack x/y>` 偏移要累加（`group.py:113-114` + `data.py:157-158`）。
#[test]
fn nested_group_offsets_accumulate() {
    let ora = tmp("nested-offset.ora");
    let png = tmp("dot.png");
    pigment_engine::surface::png::write_rgba8_png(&png, vec![0, 0, 255, 255], 1, 1).unwrap();
    let png_bytes = std::fs::read(&png).unwrap();
    let _ = std::fs::remove_file(&png);
    let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<image w="256" h="256" version="0.0.5">
  <stack>
    <stack x="64" y="64" name="G">
      <layer x="64" y="64" src="data/dot.png" opacity="1.0" visibility="visible" />
    </stack>
  </stack>
</image>"#;
    {
        let mut w = pigment_engine::zip::ZipWriter::create(&ora).unwrap();
        w.add_stored("mimetype", b"image/openraster").unwrap();
        w.add_stored("data/dot.png", &png_bytes).unwrap();
        w.add_stored("stack.xml", xml.as_bytes()).unwrap();
        w.finish().unwrap();
    }
    let mut doc = Document::new();
    doc.load(&ora).unwrap();
    // 组 64 + 层 64 = 128 → tile (2,2)
    let layer = doc.layers().layer_at(&[0, 0]).expect("组内层");
    let surface = layer.surface().expect("绘画层");
    assert!(
        surface.tile_entry(2, 2).is_some(),
        "绝对位置应是 128,128；实际 tiles={:?}",
        surface.tiles().map(|(c, _)| c).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&ora);
}

/// 画框启用时，ORA 画幅与图层坐标都相对**画框**（`document.py:1778` 传
/// `get_user_bbox()`；`_save_rect_to_ora` 用 `frame_bbox` 求 x/y）。
#[test]
fn ora_save_uses_the_frame_when_enabled() {
    let path = tmp("frame.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0); // 图层数据 bbox 从 (0,0) 起
    doc.set_frame(10, 20, 100, 80);
    doc.set_frame_enabled(true);

    let SaveOutcome::Ora { bbox } = doc.save(&path, None).unwrap() else {
        panic!("应保存为 ORA");
    };
    assert_eq!(bbox, (10, 20, 100, 80), "保存外接框 = 画框");
    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let xml = String::from_utf8(z.read("stack.xml").unwrap()).unwrap();
    assert!(
        xml.contains("w=\"100\"") && xml.contains("h=\"80\""),
        "{xml}"
    );
    // 图层数据 bbox 相对画框原点：x = layer_tile_x*64 − 10，y 同理
    let (min_tx, min_ty) = doc
        .surface()
        .tiles()
        .map(|(c, _)| c)
        .fold((i32::MAX, i32::MAX), |(x, y), (tx, ty)| {
            (x.min(tx), y.min(ty))
        });
    let x = min_tx * 64 - 10;
    let y = min_ty * 64 - 20;
    assert!(
        xml.contains(&format!("x=\"{x}\"")),
        "图层 x 应相对画框：{xml}"
    );
    assert!(
        xml.contains(&format!("y=\"{y}\"")),
        "图层 y 应相对画框：{xml}"
    );
    let _ = std::fs::remove_file(&path);
}

/// strokemap 的 tile 下标必须相对图层 bbox（`data.py:1692` 传 `-x,-y`）。
///
/// 旧实现写的是绝对坐标，单 tile 画面（原点 0）看不出来；这里把笔画放在
/// 第二行 tile，写盘后断言 `.dat` 里的 ty 是相对值，并验证载入后整体回到
/// 画幅原点（与源码 `update_frame(0,0,w,h)` 的平移一致）。
#[test]
fn ora_strokemap_tiles_are_layer_relative() {
    let path = tmp("strokemap-offset.ora");
    let mut doc = Document::new();
    paint(&mut doc, 150.0, 4.0); // y=150 → 绝对 tile y=2
    let before: Vec<(i32, i32)> = doc.surface().strokes()[0].tiles.keys().copied().collect();
    assert!(
        before.iter().any(|&(_, ty)| ty == 2),
        "笔画应落在第二行 tile"
    );

    let SaveOutcome::Ora { bbox } = doc.save(&path, None).unwrap() else {
        panic!("应保存为 ORA");
    };
    assert_eq!(bbox.1 % 64, 0, "画幅原点按 tile 对齐");

    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let dat = z.read("data/layer-00-strokemap.dat").unwrap();
    let b_len = u32::from_be_bytes(dat[1..5].try_into().unwrap()) as usize;
    let s_off = 1 + 4 + b_len;
    assert_eq!(dat[s_off], b's');
    let tx_off = s_off + 1 + 8;
    let stored_ty = i32::from_be_bytes(dat[tx_off + 4..tx_off + 8].try_into().unwrap());
    assert_eq!(stored_ty, 2 - bbox.1 / 64, "落盘 ty 应是绝对 ty 减画幅原点");

    let mut doc2 = Document::new();
    doc2.load(&path).unwrap();
    let after: Vec<(i32, i32)> = doc2.surface().strokes()[0].tiles.keys().copied().collect();
    let shifted: Vec<(i32, i32)> = before
        .iter()
        .map(|&(tx, ty)| (tx - bbox.0 / 64, ty - bbox.1 / 64))
        .collect();
    assert_eq!(after, shifted, "载入后内容整体平移到画幅原点，相对布局不变");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn ora_has_the_expected_entries_in_order() {
    let path = tmp("entries.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    doc.save(&path, None).unwrap();

    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let names: Vec<&str> = z.names().iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "mimetype",
            "data/layer-00.png",
            "data/layer-00-strokemap.dat",
            // 背景层：整幅 + 单块 tile（tree.py:2513-2526，bg_path=(len,)=(1,)）
            "data/background-01.png",
            "data/background-01-tile.png",
            "data/mypaint-settings.json",
            "Thumbnails/thumbnail.png",
            "mergedimage.png",
            "stack.xml",
        ],
        "条目顺序应照搬 _save_layers_to_new_orazip"
    );
    // mimetype 第一个且 STORED，内容精确
    assert_eq!(z.entry("mimetype").unwrap().method, 0);
    assert_eq!(z.read("mimetype").unwrap(), b"image/openraster");
    // 图层 PNG 是真正的 PNG
    let png = z.read("data/layer-00.png").unwrap();
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    // stack.xml 内容
    let xml = String::from_utf8(z.read("stack.xml").unwrap()).unwrap();
    assert!(xml.contains("src=\"data/layer-00.png\""));
    assert!(xml.contains("mypaint:version=\"2.0.0\""));
    assert!(xml.contains("mypaint:eotf=\"2.2\""));
    assert!(xml.contains("isolation=\"isolate\""));
    let _ = std::fs::remove_file(&path);
}

/// 缩略图与 mergedimage 的尺寸关系：mergedimage 跟画幅同尺寸，
/// 缩略图是减半后的尺寸。
#[test]
fn ora_preview_images_have_expected_sizes() {
    let path = tmp("previews.ora");
    let mut doc = Document::new();
    paint(&mut doc, 30.0, 6.0);
    let bbox = doc.data_bbox();
    doc.save(&path, None).unwrap();

    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let merged = z.read("mergedimage.png").unwrap();
    let (_, mw, mh) = pigment_surface::png::decode_rgba8_png(&merged).unwrap();
    assert_eq!(
        (mw as i32, mh as i32),
        (bbox.2, bbox.3),
        "mergedimage 应与画幅同尺寸"
    );

    let thumb = z.read("Thumbnails/thumbnail.png").unwrap();
    let (_, tw, th) = pigment_surface::png::decode_rgba8_png(&thumb).unwrap();
    assert!(tw <= 256 && th <= 256, "缩略图不超过 256：{tw}×{th}");
    // 小画幅不放大（scale_proportionally 是 shrink_only）
    assert!(tw as i32 <= bbox.2.max(1));
    let _ = std::fs::remove_file(&path);
}

// ---- Python 互操作 ----

/// Python `zipfile` 必须能读我们写的 `.ora`，PIL 必须能解里面的 PNG。
///
/// Python 是 MyPaint 的宿主语言，这条是"别的应用能否打开"的代理门禁。
#[test]
fn python_can_read_our_ora() {
    if !has_python() {
        eprintln!("跳过：本机没有 python");
        return;
    }
    let path = tmp("interop.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    doc.save(&path, None).unwrap();

    let script = format!(
        r#"
import zipfile, io
from PIL import Image
z = zipfile.ZipFile(r"{p}")
bad = z.testzip()
assert bad is None, bad
names = z.namelist()
assert names[0] == "mimetype", names
assert z.read("mimetype") == b"image/openraster"
# 每个 PNG 都要能被 PIL 解码
for n in ["data/layer-00.png", "Thumbnails/thumbnail.png", "mergedimage.png"]:
    im = Image.open(io.BytesIO(z.read(n)))
    im.load()
# stack.xml 必须是合法 XML
import xml.etree.ElementTree as ET
root = ET.fromstring(z.read("stack.xml"))
assert root.tag == "image", root.tag
layer = root.find("stack/layer")
assert layer is not None and layer.attrib["src"] == "data/layer-00.png"
print("INTEROP_OK")
"#,
        p = path.display()
    );
    let out = run_python(&script).expect("Python 互操作脚本失败");
    assert!(out.contains("INTEROP_OK"), "{out}");
    let _ = std::fs::remove_file(&path);
}

// ---- 读真实 MyPaint 文件 ----

fn ref_tests_dir() -> Option<PathBuf> {
    // 仓库布局：<repo>/pigment/crates/pigment-engine/tests/xxx.rs
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = here.parent()?.parent()?.parent()?;
    let d = repo.join("_mypaint_ref").join("tests");
    if d.is_dir() {
        Some(d)
    } else {
        None
    }
}

/// 读 MyPaint 自己产出的 `smallimage.ora`（老格式：`layer000.png` 命名、
/// 带 legacy `mypaint_strokemap`、有背景层）。
#[test]
fn loads_real_mypaint_smallimage() {
    let Some(dir) = ref_tests_dir() else {
        eprintln!("跳过：找不到 _mypaint_ref/tests");
        return;
    };
    let f = dir.join("smallimage.ora");
    if !f.is_file() {
        eprintln!("跳过：{f:?} 不存在");
        return;
    }
    let mut doc = Document::new();
    let loaded = doc.load(&f).unwrap();
    let LoadOutcome::Ora { bbox, .. } = loaded else {
        panic!("应识别为 ORA");
    };
    assert_eq!((bbox.2, bbox.3), (128, 128), "smallimage 画幅是 128×128");
    assert!(
        doc.surface().tile_count() > 0,
        "应读出实际像素（第一层有内容）"
    );
    // 老文件没有 mypaint:version / eotf
    assert_eq!(doc.xres, None);
}

/// 读 `fill_outlines.ora`（新格式：`layer-00-00.png` 命名、命名空间属性、
/// `data/mypaint-settings.json`）。
#[test]
fn loads_real_mypaint_fill_outlines() {
    let Some(dir) = ref_tests_dir() else {
        eprintln!("跳过：找不到 _mypaint_ref/tests");
        return;
    };
    let f = dir.join("fill_outlines.ora");
    if !f.is_file() {
        eprintln!("跳过：{f:?} 不存在");
        return;
    }
    let mut doc = Document::new();
    let loaded = doc.load(&f).unwrap();
    let LoadOutcome::Ora {
        bbox,
        compatibility,
        ..
    } = loaded
    else {
        panic!("应识别为 ORA");
    };
    assert_eq!((bbox.2, bbox.3), (3456, 3008));
    // 载入后当前层可能落在组上（fill_outlines 的顶层是组）→ 用整棵树判内容
    let walked = doc.layers().walk();
    assert!(
        walked
            .iter()
            .any(|(_, l)| l.surface().map(|s| s.tile_count() > 0) == Some(true)),
        "应读出实际像素"
    );
    assert!(
        walked.iter().any(|(_, l)| l.is_group()),
        "组结构要保留（fill_outlines 有嵌套 stack）"
    );
    assert!(
        walked.iter().any(|(p, _)| p.len() > 1),
        "至少有一层嵌在组里"
    );
    // 该文件的 mypaint:version 若存在，必须能被判定（不能 panic）
    let _ = compatibility;
    // 它带 data/mypaint-settings.json，应被读进 settings
    assert!(
        !doc.settings.is_empty(),
        "fill_outlines.ora 带设置 JSON，应被载入"
    );
}

/// 读 MyPaint 的 `bigimage.ora`（大画幅，验证不会爆内存/尺寸正确）。
#[test]
fn loads_real_mypaint_bigimage() {
    let Some(dir) = ref_tests_dir() else {
        eprintln!("跳过：找不到 _mypaint_ref/tests");
        return;
    };
    let f = dir.join("bigimage.ora");
    if !f.is_file() {
        eprintln!("跳过：{f:?} 不存在");
        return;
    }
    let mut doc = Document::new();
    let loaded = doc.load(&f).unwrap();
    let LoadOutcome::Ora { bbox, .. } = loaded else {
        panic!("应识别为 ORA");
    };
    assert!(bbox.2 > 0 && bbox.3 > 0, "画幅应为正：{bbox:?}");
    assert!(doc.surface().tile_count() > 0);
}

// ---- B2-4：多图层 ORA 往返 ----

/// 两层（不同模式/不透明度/名字/可见性）+ 背景色 + 选中层的完整往返。
#[test]
fn ora_round_trips_layer_stack() {
    let path = tmp("layers.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0); // 第 0 层（顶层）有内容

    use pigment_surface::combine::CombineMode;
    use pigment_surface::layer::LayerMode;
    // 再加一层：空的、改名、半透明、Multiply、隐藏、锁定
    assert!(doc.add_layer());
    doc.rename_layer(&[0], Some("Top".to_string()));
    doc.set_layer_opacity(&[0], 0.5);
    doc.set_layer_mode(&[0], LayerMode::Standard(CombineMode::Multiply));
    doc.set_layer_visible(&[0], false);
    doc.set_layer_locked(&[0], true);
    // 背景改成非默认色
    doc.layers_mut().set_background_color([24, 48, 96]);

    doc.save(&path, None).unwrap();

    let mut d2 = Document::new();
    let loaded = d2.load(&path).unwrap();
    let LoadOutcome::Ora { layer_count, .. } = loaded else {
        panic!("应识别为 ORA");
    };
    assert_eq!(layer_count, 2);

    // 顶层（index 0）属性
    let top = d2.layers().layer(0);
    assert_eq!(top.name.as_deref(), Some("Top"));
    assert_eq!(top.opacity, 0.5);
    assert_eq!(top.mode, LayerMode::Standard(CombineMode::Multiply));
    assert!(!top.visible);
    assert!(top.locked);
    assert_eq!(
        d2.layers().current_index(),
        0,
        "选中层应恢复到 save 时的当前层"
    );
    // 底层像素还在
    assert!(
        d2.layers().layer(1).surface().unwrap().tile_count() > 0,
        "底层内容应随文件回来"
    );
    assert!(d2.layers().layer(1).visible && !d2.layers().layer(1).locked);
    // 背景色
    assert_eq!(d2.layers().background_color(), [24, 48, 96]);
    let _ = std::fs::remove_file(&path);
}

/// 直通组保存：只丢 opacity/composite-op，`visibility` 必须保留
/// （`group.py:516-524` + `core.py:832-839`）。
#[test]
fn pass_through_group_keeps_visibility() {
    use pigment_surface::layer::LayerMode;
    let path = tmp("pass-through.ora");
    let mut doc = Document::new();
    assert!(doc.add_group());
    let p = doc.current_path().to_vec();
    doc.rename_layer(&p, Some("PT".to_string()));
    doc.set_layer_mode(&p, LayerMode::PassThrough);
    doc.set_layer_visible(&p, false);
    doc.save(&path, None).unwrap();

    let z = pigment_engine::zip::ZipArchive::open(&path).unwrap();
    let xml = String::from_utf8(z.read("stack.xml").unwrap()).unwrap();
    let line = xml
        .lines()
        .find(|l| l.contains("<stack ") && l.contains("name=\"PT\""))
        .expect("应有直通组");
    assert!(line.contains("visibility=\"hidden\""), "{line}");
    assert!(line.contains("isolation=\"auto\""), "{line}");
    assert!(!line.contains("opacity="), "直通组不写 opacity：{line}");
    assert!(
        !line.contains("composite-op="),
        "直通组不写 composite-op：{line}"
    );

    let mut d2 = Document::new();
    d2.load(&path).unwrap();
    let g = d2.layers().layer_at(&p).unwrap();
    assert!(!g.visible, "载入后仍应隐藏");
    // 源码本身也不往返直通：缺 composite-op → `core.py:169` 回落
    // default_mode() = SpectralWGM，读回是隔离组（见 B2 记账）。
    assert!(matches!(
        g.mode,
        LayerMode::Standard(pigment_surface::combine::CombineMode::SpectralWGM)
    ));
    let _ = std::fs::remove_file(&path);
}

/// 载入真实 MyPaint 文件时应把「有内容的最顶层」之外的层也全部读进来，
/// 且背景色来自背景 tile（fill_outlines.ora 的背景是白）。
#[test]
fn real_files_load_all_layers() {
    let Some(dir) = ref_tests_dir() else {
        eprintln!("跳过：找不到 _mypaint_ref/tests");
        return;
    };
    let f = dir.join("fill_outlines.ora");
    if !f.is_file() {
        eprintln!("跳过：{f:?} 不存在");
        return;
    }
    let mut doc = Document::new();
    doc.load(&f).unwrap();
    // fill_outlines.ora 的 stack 里有 12 个 layer 元素（背景元素被吸收成背景色），
    // 其中若干嵌在 3 个嵌套 stack 组里 → 按 walk() 数整棵树。
    let walked = doc.layers().walk();
    assert!(
        walked.len() >= 10,
        "应读出全部图层（含组内子层），实际 {}",
        walked.len()
    );
    assert!(walked.iter().any(|(_, l)| l.is_group()), "组要保留");
    assert!(
        walked.iter().any(|(p, _)| p.len() > 1),
        "至少有一层嵌在组里（fill_outlines 有嵌套 stack）"
    );
    // fill_outlines.ora 的背景 tile 是 64×64 的图案（白底 + 几块彩色），
    // 源码会把它当循环平铺图加载（`Background` 接受 64 倍数），不是纯色。
    let bg = doc
        .layers()
        .background_image()
        .expect("fill_outlines 的背景应是 64×64 图案");
    assert_eq!((bg.width(), bg.height()), (64, 64));
}

/// B2-6：`<stack>` 组在载入时**保留结构**（不再是平铺），嵌套层带父组。
#[test]
fn nested_stacks_keep_structure_on_load() {
    let Some(dir) = ref_tests_dir() else {
        eprintln!("跳过：找不到 _mypaint_ref/tests");
        return;
    };
    let f = dir.join("fill_outlines.ora");
    if !f.is_file() {
        eprintln!("跳过：{f:?} 不存在");
        return;
    }
    let xml_bytes = {
        let z = pigment_engine::zip::ZipArchive::open(&f).unwrap();
        z.read("stack.xml").unwrap()
    };
    let xml = String::from_utf8_lossy(&xml_bytes).into_owned();
    let img = pigment_engine::fileio::parse_stack_xml(&xml).unwrap();
    // 根 stack 的下标 0 是不是组随样本而定；但至少要有一棵嵌套的子树
    let has_group = img.layers.iter().any(|l| l.is_group);
    assert!(has_group, "样本里有 <stack> 组，解析后应保留 is_group");
    // 背景元素在根 stack 的子层里（含 bg_tile 属性）
    assert!(img.layers.iter().any(|l| l.bg_tile.is_some()));

    // 载入文档后，walk() 能给出嵌套路径，且组自身的 mode/isolation 对得上
    let mut doc = Document::new();
    doc.load(&f).unwrap();
    let walked = doc.layers().walk();
    assert!(walked.iter().any(|(_, l)| l.is_group()));
    assert!(walked.iter().any(|(p, _)| p.len() > 1), "应有嵌在组里的层");
}

/// 背景_tile 非纯色时按普通图层载入（BackgroundError 的 fallthrough）。
#[test]
fn nonuniform_background_becomes_a_layer() {
    let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<image h="64" w="64">
  <stack>
    <layer background_tile="data/grad.png" name="bg?" opacity="1.0" src="data/grad.png" x="0" y="0" />
  </stack>
</image>
"#;
    // 只验证解析层把 bg_tile 收进来（zip 侧的 fallthrough 在集成路径覆盖）
    let img = pigment_engine::fileio::parse_stack_xml(xml).unwrap();
    assert_eq!(img.layers.len(), 1);
    assert_eq!(img.layers[0].bg_tile.as_deref(), Some("data/grad.png"));
}

// ---- 与撤销的交互 ----

/// 载入之后撤销栈必须清空（源码 `load` 末行 `command_stack.clear()`）。
#[test]
fn load_clears_the_undo_stack() {
    let path = tmp("undoclear.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    doc.save(&path, None).unwrap();
    assert!(doc.command_stack.undo_len() > 0);

    // 在同一个文档上载入，栈必须被清空
    doc.load(&path).unwrap();
    assert_eq!(doc.command_stack.undo_len(), 0);
    assert_eq!(doc.command_stack.redo_len(), 0);
    let _ = std::fs::remove_file(&path);
}

/// 往返两次必须稳定：第二次往返之后**逐位**不动（量化损失只在第一次发生）。
#[test]
fn double_round_trip_is_stable() {
    let p1 = tmp("stable1.ora");
    let p2 = tmp("stable2.ora");
    let mut doc = Document::new();
    paint(&mut doc, 40.0, 4.0);
    doc.save(&p1, None).unwrap();

    let mut d2 = Document::new();
    d2.load(&p1).unwrap();
    let (before, _) = relative_tiles(d2.surface());
    d2.save(&p2, None).unwrap();

    let mut d3 = Document::new();
    d3.load(&p2).unwrap();
    let (after, _) = relative_tiles(d3.surface());
    assert_eq!(
        after, before,
        "第二次往返必须逐位一致（8 位量化只在第一次发生，之后是定点）"
    );
    let _ = std::fs::remove_file(&p1);
    let _ = std::fs::remove_file(&p2);
}
