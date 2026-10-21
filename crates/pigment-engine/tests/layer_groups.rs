//! B2-6 组嵌套（`lib/layer/group.py`）的端到端门禁。
//!
//! 覆盖：
//! 1. 结构：`walk()` 的路径、`branch_visible` / `child_count`
//! 2. 属性：直通组强制 opacity=1 且之后不可改（`core.py:274-293` / `:473-494`）
//! 3. 绘制：当前层是组时不产生作画（`data.py:1364 get_paintable`）
//! 4. 显隐：隐藏组要抬高 GPU 水位线、子层不可画
//! 5. `.ora`：嵌套 `<stack>` + `isolation`（隔离/直通）往返保真
//! 6. 合并下行：组当一层被摊平，撤销恢复结构
//! 7. 拖动重排：`RestackLayer` 搬进/搬出组

use std::path::PathBuf;

use pigment_engine::brushlib::default_brush;
use pigment_engine::hokusai::{BrushSetting, BrushState};
use pigment_engine::{Brushwork, Document, StrokeEvent};
use pigment_surface::combine::DEFAULT_MODE;
use pigment_surface::layer::{Layer, LayerMode};

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("pigment-groups-{}-{}", std::process::id(), name));
    p
}

/// 在**当前可画层**上画一条斜线（带真实位移，保证落 dab）。
fn paint(doc: &mut Document, y: f32) {
    let brush = {
        let mut b = default_brush();
        b.get_mut(BrushSetting::Radius).base_value = 3.0;
        b
    };
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    for i in 0..40 {
        let ev = StrokeEvent::new(0.016, i as f32 * 2.0, y, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0);
        let Some(surface) = doc.paintable_surface_mut() else {
            return;
        };
        bw.stroke_to(surface, &brush, "Fan#1", &mut state, ev);
    }
    if let Some(surface) = doc.paintable_surface_mut() {
        if bw.stop_recording(surface, &state, false) {
            doc.do_brushwork(bw);
        }
    }
}

/// 由名字找路径（结构在命令里会动，按名字更稳）。
fn path_of(doc: &Document, name: &str) -> Vec<usize> {
    doc.layers()
        .walk()
        .into_iter()
        .find(|(_, l)| l.name.as_deref() == Some(name))
        .map(|(p, _)| p)
        .unwrap_or_else(|| panic!("找不到层 {name}"))
}

/// 直接搭出一棵确定的树（不经过命令，避免 add/insert 的落位歧义）：
///
/// 顶层 = `[G(A, B), C, L0]`；路径：G=[0]、A=[0,0]、B=[0,1]、C=[1]、L0=[2]。
fn build_tree() -> Document {
    let mut doc = Document::new();
    {
        let mut a = Layer::new();
        a.name = Some("A".into());
        let mut b = Layer::new();
        b.name = Some("B".into());
        let mut g = Layer::new_group();
        g.name = Some("G".into());
        {
            let st = g.stack_mut().unwrap();
            st.insert(0, a);
            st.insert(1, b);
        }
        let mut c = Layer::new();
        c.name = Some("C".into());
        let layers = doc.layers_mut();
        layers.insert(0, c);
        layers.insert(0, g);
    }
    doc.select_layer(&[0]);
    doc
}

#[test]
fn group_structure_and_paths() {
    let doc = build_tree();
    let walked = doc.layers().walk();
    let paths: Vec<Vec<usize>> = walked.iter().map(|(p, _)| p.clone()).collect();
    // 组在前、子层紧跟自己的组（`tree.py:1580` 的 `queue[:0] = children`）
    assert_eq!(
        paths,
        vec![vec![0], vec![0, 0], vec![0, 1], vec![1], vec![2]],
        "walk 序 = 组 → 子层 → 下一个兄弟"
    );
    assert!(doc.layers().layer_at(&[0]).unwrap().is_group());
    assert_eq!(doc.layers().layer_at(&[0]).unwrap().child_count(), 2);
    assert_eq!(
        doc.layers().layer_at(&[0]).unwrap().mode,
        LayerMode::Standard(DEFAULT_MODE),
        "新组默认是隔离模式（SpectralWGM）"
    );
    // branch_visible：祖先可见 → true
    assert!(doc.layers().branch_visible(&[0, 0]));
    // 当前层 = 组
    assert_eq!(doc.current_path(), &[0]);
}

/// 直通组：组自身的 opacity 被强制 1.0，且不可再改（`core.py:274-293`）。
#[test]
fn pass_through_forces_opacity_and_undo_restores() {
    let mut doc = build_tree();
    doc.set_layer_opacity(&[0], 0.4);
    assert_eq!(doc.layers().layer_at(&[0]).unwrap().opacity, 0.4);
    doc.set_layer_mode(&[0], LayerMode::PassThrough);
    let g = doc.layers().layer_at(&[0]).unwrap();
    assert_eq!(g.mode, LayerMode::PassThrough);
    assert_eq!(g.opacity, 1.0, "直通模式强制 opacity=1（core.py:483）");
    // 再改 opacity 不生效
    doc.set_layer_opacity(&[0], 0.2);
    assert_eq!(doc.layers().layer_at(&[0]).unwrap().opacity, 1.0);
    // 空改动的 SetLayerOpacity 也入了栈（源码同：redo 里设置器拒绝，但命令照样入栈）
    doc.undo(); // 撤销那次被拒的 opacity（无效果）
                // 再撤销 mode → 回到隔离；撤销顺序照 command.py:1557-1560（先 mode 后 opacity）
    doc.undo();
    let g = doc.layers().layer_at(&[0]).unwrap();
    assert_eq!(g.mode, LayerMode::Standard(DEFAULT_MODE));
    assert_eq!(g.opacity, 0.4, "撤销把旧 opacity 也带回来");
}

/// 当前层是组时不能画（`data.py:1364 get_paintable`）。
#[test]
fn painting_on_a_group_does_nothing() {
    let mut doc = build_tree();
    doc.select_layer(&[0]); // 组
    assert!(doc.paintable_surface_mut().is_none());
    let before = doc.command_stack.undo_len();
    paint(&mut doc, 20.0);
    assert_eq!(doc.command_stack.undo_len(), before, "组上作画不产生命令");
    // 组内的子层可以画
    doc.select_layer(&[0, 0]);
    assert!(doc.paintable_surface_mut().is_some());
    paint(&mut doc, 20.0);
    assert!(doc.command_stack.undo_len() > before, "子层上作画应入栈");
    assert!(
        doc.layers()
            .layer_at(&[0, 0])
            .unwrap()
            .surface()
            .unwrap()
            .tile_count()
            > 0
    );
}

/// 隐藏组 → 子层不可画、也要抬高合成版本号（GPU 水位线）。
#[test]
fn hidden_group_hides_children_in_composite_and_paintable() {
    let mut doc = build_tree();
    doc.select_layer(&[0, 0]);
    paint(&mut doc, 10.0);
    let before = doc.layers().composite_revision();
    doc.set_layer_visible(&[0], false);
    assert_ne!(
        doc.layers().composite_revision(),
        before,
        "隐藏组也要抬高 GPU 水位线"
    );
    doc.select_layer(&[0, 0]);
    assert!(
        doc.paintable_surface_mut().is_none(),
        "祖先隐藏时子层不可画"
    );
    // 组的子层在 walk 里仍可见（列表要照常显示）
    assert!(doc.layers().layer_at(&[0, 1]).is_some());
}

/// `.ora` 往返：嵌套 `<stack>` 结构、isolation（隔离/直通）与组属性都要保真。
#[test]
fn ora_round_trips_group_structure() {
    let mut doc = build_tree();
    doc.set_layer_opacity(&[0], 0.7);
    // 第二个组：直通（add_group 落在当前层位置 → 组 G 变成 [1]）
    doc.select_layer(&[0]);
    doc.add_group();
    let pt_path = doc.current_path().to_vec();
    doc.set_layer_mode(&pt_path, LayerMode::PassThrough);
    doc.rename_layer(&pt_path, Some("PT".to_string()));
    doc.rename_layer(&path_of(&doc, "G"), Some("G".to_string()));

    let path = tmp("groups.ora");
    doc.save(&path, None).unwrap();

    let mut back = Document::new();
    back.load(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    // 隔离组 G：仍是 SpectralWGM、opacity 0.7、两个子层
    let g = path_of(&back, "G");
    let g = back.layers().layer_at(&g).unwrap();
    assert!(g.is_group(), "组结构要保留");
    assert_eq!(g.mode, LayerMode::Standard(DEFAULT_MODE));
    assert!(
        (g.opacity - 0.7).abs() < 1e-3,
        "组不透明度要往返：{}",
        g.opacity
    );
    assert_eq!(g.child_count(), 2, "子层数不变");
    // 直通组：写 isolation="auto"、丢 opacity/composite-op → 读回仍是直通
    let pt = path_of(&back, "PT");
    let pt = back.layers().layer_at(&pt).unwrap();
    // ⚠️ 源码的**真实行为**：pass-through 保存时丢掉 composite-op，而载入端的`r`n    // `_load_common_flags_from_ora_elem`（core.py:169-170）对缺失属性回落到`r`n    // `default_mode()` = SpectralWGM，不是 ORA 规范里的 src-over → 读回来是`r`n    // **隔离组**。这个不对称照搬（`group.py:519-523` 的注释与 `core.py:169` 对不上，`r`n    // 属于 MyPaint 自身行为），别"顺手修掉"。
    assert_eq!(
        pt.mode,
        LayerMode::Standard(DEFAULT_MODE),
        "pass-through 不参与 ORA 往返（源码同）"
    );
    // 子层的名字也回来了
    assert!(back
        .layers()
        .walk()
        .iter()
        .any(|(_, l)| l.name.as_deref() == Some("A")));
}

/// 合并下行：当前层与**下一个兄弟**合并；组会被摊平成绘画层。
#[test]
fn merge_down_flattens_a_group_sibling() {
    let mut doc = build_tree();
    doc.select_layer(&[0]); // 组 G，下一个兄弟是 C
    let before = doc.layers().walk().len();
    assert!(doc.merge_layer_down(), "应当可合并（组可归一化）");
    let walked = doc.layers().walk();
    assert_eq!(
        walked.len(),
        before - 3,
        "组与 C 并成一层（组内 2 层 + 组 + C → 合并层 + L0）"
    );
    let merged_path = doc
        .layers()
        .walk()
        .into_iter()
        .map(|(p, _)| p)
        .find(|p| p.len() == 1)
        .expect("有顶层层");
    let merged = doc.layers().layer_at(&merged_path).unwrap();
    assert!(!merged.is_group(), "合并结果是绘画层");
    // 撤销复原结构
    doc.undo();
    assert_eq!(doc.layers().walk().len(), before, "撤销要恢复组结构");
    assert!(doc.layers().layer_at(&[0]).unwrap().is_group());
}

/// 拖动重排：把层搬进组、再搬出来（`RestackLayer` 的路径语义）。
#[test]
fn restack_moves_layer_into_and_out_of_group() {
    let mut doc = build_tree();
    let c = path_of(&doc, "C");
    assert_eq!(c, vec![1]);
    assert!(doc.restack_layer(c.clone(), vec![0, 0]));
    assert_eq!(doc.layers().layer_at(&[0]).unwrap().child_count(), 3);
    assert_eq!(
        doc.layers().layer_at(&[0, 0]).unwrap().name.as_deref(),
        Some("C"),
        "插到组顶"
    );
    // 撤销 → 回到顶层原位
    doc.undo();
    assert_eq!(
        doc.layers().layer_at(&c).unwrap().name.as_deref(),
        Some("C"),
        "撤销把它放回原处"
    );
    assert_eq!(doc.layers().layer_at(&[0]).unwrap().child_count(), 2);
}
