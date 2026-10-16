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
type RelativeTiles = (std::collections::BTreeMap<(i32, i32), TilePixels>, (i32, i32));

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
fn assert_pixels_match(a: &pigment_surface::PigmentSurface, b: &pigment_surface::PigmentSurface, whose: &str) {
    /// 8 位下允许的最大偏差。见上：alpha 量化 + EOTF 近零处陡斜率。
    const TOL: i32 = 16;
    let (ma, _) = relative_tiles(a);
    let (mb, _) = relative_tiles(b);
    let bg_lin = pigment_surface::color::bg_to_linear(pigment_surface::color::CANVAS_BG);
    let empty = [[[0u16; 4]; 64]; 64];
    let bg_px = pigment_surface::color::tile_to_rgba8_single([0, 0, 0, 0], pigment_surface::color::CANVAS_BG, bg_lin);

    let mut keys: Vec<(i32, i32)> = ma.keys().chain(mb.keys()).copied().collect();
    keys.sort_unstable();
    keys.dedup();
    assert!(!keys.is_empty(), "{whose}：两边都没有内容");

    for k in keys {
        let pa = ma.get(&k).map(|p| &**p).unwrap_or(&empty);
        let pb = mb.get(&k).map(|p| &**p).unwrap_or(&empty);
        for y in 0..64 {
            for x in 0..64 {
                let va = pigment_surface::color::tile_to_rgba8_single(pa[y][x], pigment_surface::color::CANVAS_BG, bg_lin);
                let vb = pigment_surface::color::tile_to_rgba8_single(pb[y][x], pigment_surface::color::CANVAS_BG, bg_lin);
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

fn has_python() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_python(script: &str) -> Result<String, String> {
    let out = std::process::Command::new("python")
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
    assert_eq!(
        (lb.2, lb.3),
        (bbox.2, bbox.3),
        "往返后画幅尺寸应一致"
    );
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
    assert!(doc.save(tmp("x.jpg"), None).is_err(), "jpg 尚未移植");
}

#[test]
fn loading_missing_file_reports_not_found() {
    let mut doc = Document::new();
    assert!(doc.load(tmp("definitely-missing-xyz.ora")).is_err());
    assert!(doc.load(tmp("definitely-missing-xyz.png")).is_err());
}

// ---- ORA 容器结构 ----

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
    assert!(doc.surface().tile_count() > 0, "应读出实际像素");
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
