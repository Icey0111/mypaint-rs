//! 撤销/重做的端到端门禁 —— 端到端模拟应用层的「画 → 撤销 → 重做」循环。
//!
//! 这里用**官方 `.myb` 笔刷**（不是造出来的测试笔）跑完整链路：
//! `Brushwork` 记录 → `Document` 提交 → `undo()`/`redo()` 切快照，
//! 每一步都逐像素核对，确保撤销真的把画布退回原状、重做真的逐像素还原。
//!
//! 与 `command.rs` 的单测区别：那些用合成笔刷测栈的语义，这里测**真实笔刷
//! 在真实文档上的像素结果**，是 P0「撤销/重做」的验收门禁。

use pigment_engine::brushlib::default_brush;
use pigment_engine::hokusai::tile::TILE_SIZE;
use pigment_engine::hokusai::{Brush, BrushState};
use pigment_engine::{Brushwork, CommandUpdate, Document, StrokeEvent};
use pigment_surface::PigmentSurface;

/// 真实笔刷，半径加大便于稳定地覆盖多个 tile。
fn real_brush() -> Brush {
    let mut b = default_brush();
    b.get_mut(pigment_engine::hokusai::BrushSetting::Radius)
        .base_value = 3.0;
    b
}

/// 在文档上画一条水平线并提交；返回是否真的改了画面。
fn paint_line(doc: &mut Document, y: f32, radius: f32) -> bool {
    let mut brush = real_brush();
    brush
        .get_mut(pigment_engine::hokusai::BrushSetting::Radius)
        .base_value = radius;
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    for i in 0..60 {
        let ev = StrokeEvent::new(0.016, i as f32 * 1.5, y, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        bw.stroke_to(doc.surface_mut(), &brush, "Fan#1", &mut state, ev);
    }
    let changed = bw.stop_recording(doc.surface_mut(), &state, false);
    if changed {
        doc.do_brushwork(bw);
    }
    changed
}

/// 一块 tile 的像素（64×64 RGBA fix15）。
type TilePixels = Box<[[[u16; 4]; TILE_SIZE]; TILE_SIZE]>;

/// 把整个表面的像素抓成可比较的快照（坐标 → 数据）。
fn capture(surface: &PigmentSurface) -> Vec<((i32, i32), TilePixels)> {
    let mut v: Vec<_> = surface
        .tiles()
        .map(|(c, t)| (c, Box::new(*t.pixels())))
        .collect();
    v.sort_by_key(|(c, _)| *c);
    v
}

#[test]
fn undo_restores_pixels_then_redo_reproduces_them() {
    let mut doc = Document::new();

    assert!(paint_line(&mut doc, 20.0, 3.0), "第一笔应画出内容");
    let after_first = capture(doc.surface());
    assert!(!after_first.is_empty());

    assert!(paint_line(&mut doc, 120.0, 3.0), "第二笔应画出内容");
    let after_second = capture(doc.surface());
    assert!(
        after_second.len() > after_first.len(),
        "第二笔应铺到更多 tile"
    );
    assert_eq!(doc.command_stack.undo_len(), 2);

    // 撤销第二笔 → 精确回到第一笔后的状态
    let name = doc.undo().expect("应有可撤销的命令");
    assert!(name.ends_with("of painting with Fan#1"), "{name}");
    assert_eq!(capture(doc.surface()), after_first, "撤销必须逐像素还原");
    assert_eq!(doc.command_stack.undo_len(), 1);
    assert_eq!(doc.command_stack.redo_len(), 1);

    // 重做第二笔 → 精确回到两笔后的状态
    doc.redo().expect("应有可重做的命令");
    assert_eq!(capture(doc.surface()), after_second, "重做必须逐像素还原");
    assert_eq!(doc.command_stack.redo_len(), 0);
}

#[test]
fn undo_all_the_way_back_to_an_empty_canvas() {
    let mut doc = Document::new();
    for i in 0..5 {
        assert!(paint_line(&mut doc, i as f32 * 70.0, 3.0));
    }
    assert!(doc.surface().tile_count() > 0);

    for _ in 0..5 {
        doc.undo();
    }
    assert_eq!(doc.surface().tile_count(), 0, "全部撤销后画布应为空");
    assert_eq!(doc.unsaved_painting_time, 0.0);

    // 逐步重做回去
    for i in 0..5 {
        doc.redo();
        assert!(
            doc.surface().tile_count() > 0,
            "重做第 {} 笔后应重新有内容",
            i + 1
        );
    }
}

/// 撤销后画一笔新的：redo 分支被丢弃（对齐 Krita 行为）。
#[test]
fn painting_after_undo_discards_the_redo_branch() {
    let mut doc = Document::new();
    paint_line(&mut doc, 20.0, 3.0);
    paint_line(&mut doc, 120.0, 3.0);
    doc.undo();
    assert_eq!(doc.command_stack.redo_len(), 1);

    paint_line(&mut doc, 220.0, 3.0);
    assert_eq!(doc.command_stack.redo_len(), 0, "新命令必须清掉 redo 分支");
    assert!(doc.redo().is_none(), "redo 分支已丢弃，无可重做");
}

/// 栈深上限：超出后最旧的命令被丢弃，但画布内容保持在最新状态。
#[test]
fn undo_stack_is_trimmed_at_the_configured_depth() {
    let mut doc = Document::with_max_undo_stack_size(3);
    for i in 0..6 {
        paint_line(&mut doc, i as f32 * 70.0, 3.0);
    }
    assert_eq!(doc.command_stack.undo_len(), 3, "栈深应被修剪到 3");
    let full = capture(doc.surface());

    // 只撤得掉最后三笔
    for _ in 0..3 {
        assert!(doc.undo().is_some());
    }
    assert!(doc.undo().is_none(), "更早的命令已被丢弃");
    assert!(
        doc.surface().tile_count() > 0,
        "前三笔已不在栈上，画布上应仍有它们的内容"
    );
    let _ = full;
}

/// 撤销 → 重做 → 撤销 的往复必须稳定（反复切换不出漂移）。
#[test]
fn repeated_undo_redo_cycles_are_stable() {
    let mut doc = Document::new();
    paint_line(&mut doc, 20.0, 3.0);
    let base = capture(doc.surface());
    paint_line(&mut doc, 120.0, 4.0);
    let top = capture(doc.surface());

    for round in 0..6 {
        doc.undo();
        assert_eq!(capture(doc.surface()), base, "第 {round} 轮撤销漂移了");
        doc.redo();
        assert_eq!(capture(doc.surface()), top, "第 {round} 轮重做漂移了");
    }
}

/// `load_snapshot` 会移除撤销后消失的 tile —— 渲染层的显存剔除依据。
#[test]
fn undo_reports_tiles_to_evict_from_the_gpu_mirror() {
    let mut doc = Document::new();
    paint_line(&mut doc, 20.0, 3.0);
    let before: Vec<(i32, i32)> = doc.surface().tiles().map(|(c, _)| c).collect();
    assert!(!before.is_empty());

    // 快照 → 再画一笔更宽的 → 回滚。新增的 tile 必须被报告为"需剔除"。
    let sshot = doc.surface().save_snapshot();
    let added = paint_line(&mut doc, 400.0, 4.0);
    assert!(added);
    let grown: Vec<(i32, i32)> = doc.surface().tiles().map(|(c, _)| c).collect();
    assert!(grown.len() > before.len(), "第二笔应新增 tile");

    let diff = doc.surface_mut().load_snapshot(&sshot);
    assert!(!diff.removed.is_empty(), "撤销应报告需要剔除的 tile");

    let after: Vec<(i32, i32)> = doc.surface().tiles().map(|(c, _)| c).collect();
    let mut a = after.clone();
    let mut b = before.clone();
    a.sort_unstable();
    b.sort_unstable();
    assert_eq!(a, b, "快照回滚后 tile 集合应精确复原");
}

/// `update_last_command`：换笔重绘上一笔，结果与"用新笔重画一遍"一致。
#[test]
fn live_update_equivalent_to_repainting_with_the_new_brush() {
    let mut doc = Document::new();
    paint_line(&mut doc, 20.0, 2.0);
    paint_line(&mut doc, 20.0, 2.0); // 两笔同位置，只更新最后一笔

    // A：真实路径 —— update 换笔重绘
    let mut a = doc;
    let mut fatter = real_brush();
    fatter
        .get_mut(pigment_engine::hokusai::BrushSetting::Radius)
        .base_value = 5.0;
    let name = a.update_last_command(&CommandUpdate::Brush {
        brush: &fatter,
        name: "Fatter",
    });
    assert!(name.is_some());
    let via_update = capture(a.surface());

    // B：对照组 —— 撤销后用新笔重画
    let mut b = Document::new();
    paint_line(&mut b, 20.0, 2.0);
    {
        // 直接重放同一笔的新版本
        let mut state = BrushState::default();
        let mut bw = Brushwork::new();
        for i in 0..60 {
            let ev = StrokeEvent::new(0.016, i as f32 * 1.5, 20.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
            bw.stroke_to(b.surface_mut(), &fatter, "Fatter", &mut state, ev);
        }
        if bw.stop_recording(b.surface_mut(), &state, false) {
            b.do_brushwork(bw);
        }
    }
    let via_repaint = capture(b.surface());

    assert_eq!(via_update, via_repaint, "update 应与用新笔重画逐像素一致");
}

/// 极小的单 tile 笔画也要能撤销干净（边界情形）。
///
/// 注意：**引擎要累计到一定位移才产生第一个 dab**（实测完全静止时永远
/// 不画；半径 3 + 0.5px 步长需要 6 个事件才落笔，且第一个 dab 已跨到
/// 相邻 tile）。半径 2 + 4 个 0.5px 步长的事件恰好落在单个 tile 内。
#[test]
fn single_tile_stroke_undo_is_exact() {
    let mut doc = Document::new();
    let mut brush = real_brush();
    brush
        .get_mut(pigment_engine::hokusai::BrushSetting::Radius)
        .base_value = 2.0;
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    for i in 0..4 {
        let ev = StrokeEvent::new(
            0.016,
            30.0 + i as f32 * 0.5,
            30.0,
            1.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
        );
        bw.stroke_to(doc.surface_mut(), &brush, "Fan#1", &mut state, ev);
    }
    let changed = bw.stop_recording(doc.surface_mut(), &state, false);
    assert!(changed, "极短笔画也应被判定为改动了画面");
    doc.do_brushwork(bw);

    let ts = TILE_SIZE as i32;
    assert_eq!((30 / ts, 30 / ts), (0, 0));
    assert_eq!(doc.surface().tile_count(), 1, "笔画应只占一个 tile");

    doc.undo();
    assert_eq!(doc.surface().tile_count(), 0);
    doc.redo();
    assert_eq!(doc.surface().tile_count(), 1);
    assert!(doc.surface().tile(0, 0).is_some());
}

/// 完全静止的"点"不产生内容，因此不入栈（与引擎行为一致，非缺陷）。
#[test]
fn a_fully_stationary_point_paints_nothing() {
    let mut doc = Document::new();
    let brush = real_brush();
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    for _ in 0..10 {
        let ev = StrokeEvent::new(0.016, 30.0, 30.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        bw.stroke_to(doc.surface_mut(), &brush, "Fan#1", &mut state, ev);
    }
    let changed = bw.stop_recording(doc.surface_mut(), &state, false);
    assert!(!changed, "静止点不产生 dab，应被判为空");
    assert_eq!(doc.command_stack.undo_len(), 0, "空命令不入栈");
}
