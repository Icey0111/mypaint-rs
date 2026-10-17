//! 命名可见性视图 —— `lib/layervis.py` 的模型层。
//!
//! 用途（源码 docstring）：同一幅画可能有多个变体（表情/姿势/时段），
//! 用户把「当前哪些层可见」存成一个命名视图，之后一键切换；切换走撤销栈。
//!
//! 源码的数据结构是 **每层一个 `__visible_in_views` 集合** + 管理器里的视图表
//! （`layervis.py:54-434`）。这里用反向索引（每个视图存可见层的稳定 id），
//! 两者信息等价，且更适合 Rust 的借用模型：
//!
//! - 持久化格式**逐字照源码**：`layervis.views` / `active_view` /
//!   `layer_views`（层路径 → 视图名列表），见 `layervis.py:214-251`；
//! - 视图身份 = 名字（`_NamedViewsSet` 强制唯一），层身份 = **稳定 id**
//!   （源码用对象引用，序列化时才换成路径）；
//! - 激活视图时直接改各层 `visible`（`layervis.py:378-395`）—— 源码这里
//!   不产生命令，撤销由 `ActivateLayerView` 命令负责；
//! - 视图激活期间，外部改可见性会把改动记回当前视图
//!   （`_stack_layer_props_changed_cb`，`:399-412`）。我们的可见性命令不
//!   发事件，所以由 [`LayerViews::sync`] 在每次命令/撤销后统一对账。
//!
//! 未移植：`_stack_layer_inserted_cb` 的独立事件（`sync` 覆盖同样语义）。

use std::collections::BTreeSet;

use pigment_surface::LayerStack;
use serde_json::{json, Value};

/// 默认视图名 —— `lib/layervis.py:40-43` 的 `NEW_VIEW_IDENT`。
pub const NEW_VIEW_IDENT: &str = "View";
/// 未激活视图时下拉框里的显示名 —— `UNSAVED_VIEW_DISPLAY_NAME`。
pub const UNSAVED_VIEW_DISPLAY_NAME: &str = "No active view";

/// 一个命名可见性视图（`_View`，`layervis.py:54-132`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerView {
    pub name: String,
    pub locked: bool,
    /// 在这个视图里**可见**的层 id 集合。
    visible: BTreeSet<u64>,
}

impl LayerView {
    pub fn new(name: impl Into<String>, locked: bool) -> Self {
        Self {
            name: name.into(),
            locked,
            visible: BTreeSet::new(),
        }
    }

    pub fn is_visible(&self, id: u64) -> bool {
        self.visible.contains(&id)
    }
}

/// 视图管理器（`LayerViewManager`，`layervis.py:174-549`）。
#[derive(Clone, Debug, Default)]
pub struct LayerViews {
    views: Vec<LayerView>,
    active: Option<usize>,
}

impl LayerViews {
    // ---- 只读查询 ----

    pub fn names(&self) -> Vec<String> {
        self.views.iter().map(|v| v.name.clone()).collect()
    }

    pub fn current_name(&self) -> Option<&str> {
        self.active.map(|i| self.views[i].name.as_str())
    }

    pub fn current_locked(&self) -> bool {
        self.active.is_some_and(|i| self.views[i].locked)
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.views.iter().position(|v| v.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    pub fn views(&self) -> &[LayerView] {
        &self.views
    }

    // ---- 状态变更 ----

    /// 捕获当前可见层，新建一个视图并激活（`add_new_view`，`:444-470`）。
    pub fn add_new(&mut self, name: Option<&str>, layers: &mut LayerStack) -> usize {
        let base = name.filter(|n| !n.is_empty()).unwrap_or(NEW_VIEW_IDENT);
        let unique = make_unique_name(base, &self.names(), NEW_VIEW_IDENT);
        let mut view = LayerView::new(unique, true);
        for (_, layer) in layers.walk() {
            if layer.visible {
                view.visible.insert(layer.id);
            }
        }
        self.views.push(view);
        self.active = Some(self.views.len() - 1);
        self.apply_active(layers);
        self.views.len() - 1
    }

    /// 插入一个已有视图并激活（撤销 `RemoveActiveLayerView` 用）。
    pub fn insert_view(&mut self, index: usize, view: LayerView, layers: &mut LayerStack) {
        let index = index.min(self.views.len());
        self.views.insert(index, view);
        self.active = Some(index);
        self.apply_active(layers);
    }

    /// 移除当前视图，返回 `(原下标, 视图)`；`restore` 是移除后要激活的下标。
    ///
    /// 照 `remove_active_view`（`:496-511`）：**先**切到 `restore`，再删。
    pub fn remove_active(
        &mut self,
        restore: Option<usize>,
        layers: &mut LayerStack,
    ) -> Option<(usize, LayerView)> {
        let idx = self.active?;
        let view = self.views.remove(idx);
        self.active = match restore {
            Some(r) if r > idx => Some(r - 1),
            Some(r) => Some(r),
            None => None,
        };
        self.apply_active(layers);
        Some((idx, view))
    }

    /// 激活视图（`activate_view`，`:479-485`）。
    pub fn activate(&mut self, index: Option<usize>, layers: &mut LayerStack) {
        self.active = index.filter(|i| *i < self.views.len());
        self.apply_active(layers);
    }

    /// 按名字激活（`activate_view_by_name`，`:487-494`）。
    pub fn activate_by_name(&mut self, name: Option<&str>, layers: &mut LayerStack) -> bool {
        let index = match name {
            Some(n) => match self.index_of(n) {
                Some(i) => Some(i),
                None => return false,
            },
            None => None,
        };
        self.activate(index, layers);
        true
    }

    /// 重命名当前视图，返回 `(旧名, 新唯一名)`（`rename_active_view`，`:513-540`）。
    pub fn rename_active(&mut self, base: &str) -> Option<(String, String)> {
        let idx = self.active?;
        let base = if base.is_empty() { NEW_VIEW_IDENT } else { base };
        let mut others = self.names();
        others.remove(idx);
        let new_name = make_unique_name(base, &others, NEW_VIEW_IDENT);
        let old = std::mem::replace(&mut self.views[idx].name, new_name.clone());
        Some((old, new_name))
    }

    /// 设置当前视图的锁定位（`set_active_view_locked`，`:542-549`）。
    pub fn set_locked(&mut self, locked: bool) -> bool {
        let Some(idx) = self.active else {
            return false;
        };
        if self.views[idx].locked == locked {
            return false;
        }
        self.views[idx].locked = locked;
        true
    }

    /// 把当前视图的可见性应用到各层（`_current_view_changed_cb`，`:378-395`）。
    ///
    /// 源码只在状态不一致时赋值；赋值后层自己发属性变更事件。我们没有事件，
    /// 所以这里显式 `touch()` 让 GPU 合成水位线抬高。
    pub fn apply_active(&self, layers: &mut LayerStack) {
        let Some(idx) = self.active else {
            return;
        };
        let view = &self.views[idx];
        let mut changed = false;
        let ids: Vec<(u64, bool)> = layers
            .walk()
            .into_iter()
            .map(|(_, l)| (l.id, l.visible))
            .collect();
        for (id, visible) in ids {
            let want = view.visible.contains(&id);
            if visible != want {
                if let Some(layer) = layers.layer_by_id_mut(id) {
                    layer.visible = want;
                    changed = true;
                }
            }
        }
        if changed {
            layers.touch();
        }
    }

    /// 命令/撤销之后与图层树对账：
    ///
    /// - 删掉的层从所有视图的集合里剔除；
    /// - 当前视图激活期间，外部可见性改动记回该视图
    ///   （`_stack_layer_props_changed_cb` / `_stack_layer_inserted_cb`）。
    pub fn sync(&mut self, layers: &LayerStack) {
        let ids: BTreeSet<u64> = layers.walk().into_iter().map(|(_, l)| l.id).collect();
        for view in &mut self.views {
            view.visible.retain(|id| ids.contains(id));
        }
        if let Some(idx) = self.active {
            let visible_now: Vec<(u64, bool)> = layers
                .walk()
                .into_iter()
                .map(|(_, l)| (l.id, l.visible))
                .collect();
            for (id, visible) in visible_now {
                if visible {
                    self.views[idx].visible.insert(id);
                } else {
                    self.views[idx].visible.remove(&id);
                }
            }
        }
    }

    // ---- 持久化（`layervis.py:214-328`）----

    /// 转成文档 settings 里的 `layervis` 值。
    pub fn to_json(&self, layers: &LayerStack) -> Value {
        let views: Vec<Value> = self
            .views
            .iter()
            .map(|v| json!({"name": v.name, "locked": v.locked}))
            .collect();
        let by_path: Vec<Value> = layers
            .walk()
            .into_iter()
            .map(|(path, layer)| {
                let names: Vec<Value> = self
                    .views
                    .iter()
                    .filter(|v| v.visible.contains(&layer.id))
                    .map(|v| Value::String(v.name.clone()))
                    .collect();
                json!([path, names])
            })
            .collect();
        json!({
            "views": views,
            "active_view": self.current_name(),
            "layer_views": by_path,
        })
    }

    /// 从文档 settings 的 `layervis` 值重建；随后应用当前视图的可见性。
    ///
    /// 照 `_doc_settings_modified_cb`（`:253-328`）：容错，坏条目跳过。
    pub fn load_json(&mut self, value: &Value, layers: &mut LayerStack) {
        self.views.clear();
        self.active = None;
        let Some(obj) = value.as_object() else {
            return;
        };
        if let Some(list) = obj.get("views").and_then(|v| v.as_array()) {
            for item in list {
                let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                let locked = item
                    .get("locked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let unique = make_unique_name(name, &self.names(), NEW_VIEW_IDENT);
                self.views.push(LayerView::new(unique, locked));
            }
        }
        if let Some(name) = obj.get("active_view").and_then(|v| v.as_str()) {
            self.active = self.index_of(name);
        }
        if let Some(list) = obj.get("layer_views").and_then(|v| v.as_array()) {
            for item in list {
                let Some(pair) = item.as_array() else { continue };
                let (Some(path), Some(names)) = (pair.first(), pair.get(1)) else {
                    continue;
                };
                let Some(path) = path.as_array() else { continue };
                let path: Vec<usize> = path
                    .iter()
                    .filter_map(|n| n.as_u64().map(|n| n as usize))
                    .collect();
                let Some(layer) = layers.layer_at(&path) else {
                    continue;
                };
                let id = layer.id;
                let Some(names) = names.as_array() else { continue };
                for name in names.iter().filter_map(|n| n.as_str()) {
                    if let Some(idx) = self.index_of(name) {
                        self.views[idx].visible.insert(id);
                    }
                }
            }
        }
        self.apply_active(layers);
    }
}

/// `make_unique_name`（`lib/naming.py:41-87`）的移植。
///
/// 末尾的 `" N"` 会被拆成基名 + 序号；若未带序号则从 1 开始。`always_number`
/// 指定的基名（这里是 `"View"`）**第一次也要带序号**（`View 1`）。
pub fn make_unique_name(name: &str, existing: &[String], always_number: &str) -> String {
    let (base, mut num) = match name.rsplit_once(' ') {
        Some((base, n)) if !base.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (base.to_string(), n.parse::<u32>().unwrap_or(1))
        }
        _ => (name.to_string(), 1),
    };
    let mut force = name == always_number;
    let mut candidate = name.to_string();
    while existing.iter().any(|e| e == &candidate) || force {
        candidate = format!("{base} {num}");
        num += 1;
        force = false;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_surface::Layer;

    fn stack() -> LayerStack {
        let a = Layer::new();
        let mut b = Layer::new();
        b.visible = false;
        let c = Layer::new();
        LayerStack::from_children(vec![a, b, c])
    }

    #[test]
    fn unique_names_match_the_source_doctest() {
        // lib/naming.py 的 doctest
        let existing = vec!["abc 1".to_string(), "abc 2".to_string(), "abc".to_string()];
        assert_eq!(make_unique_name("abc", &existing, ""), "abc 3");
        assert_eq!(make_unique_name("abc 1", &existing, ""), "abc 3");
        assert_eq!(make_unique_name("xyz", &[], "xyz"), "xyz 1");
        assert_eq!(make_unique_name("xyz", &[], "xyz"), "xyz 1");
        assert_eq!(make_unique_name("abc 7", &existing, ""), "abc 7");
    }

    #[test]
    fn view_captures_and_applies_visibility() {
        let mut layers = stack();
        let mid = layers.walk()[1].1.id;
        let mut views = LayerViews::default();
        views.add_new(None, &mut layers); // 捕获 a/c 可见，b 隐藏
        assert_eq!(views.names(), vec!["View 1"]);
        assert_eq!(views.current_name(), Some("View 1"));
        assert!(views.current_locked());

        // 再建一个视图（两个都捕获 a/c）
        views.add_new(Some("Other"), &mut layers);
        assert_eq!(views.names(), vec!["View 1", "Other"]);

        // 当前是 Other：外部把 b 打开 → 只记回 Other
        layers.layer_by_id_mut(mid).unwrap().visible = true;
        views.sync(&layers);

        // 切回 View 1：b 应被隐藏（它没被记进 View 1）
        views.activate(Some(0), &mut layers);
        assert!(!layers.layer_by_id(mid).unwrap().visible);
        // 切到 Other：b 可见（外部改动记进了 Other）
        views.activate(Some(1), &mut layers);
        assert!(layers.layer_by_id(mid).unwrap().visible);
    }

    #[test]
    fn view_json_roundtrip_uses_paths() {
        let mut layers = stack();
        let mut views = LayerViews::default();
        views.add_new(None, &mut layers);
        let json = views.to_json(&layers);
        assert_eq!(json["active_view"], "View 1");
        assert_eq!(json["layer_views"].as_array().unwrap().len(), 3);

        let mut rebuilt = LayerViews::default();
        rebuilt.load_json(&json, &mut layers);
        assert_eq!(rebuilt.names(), vec!["View 1"]);
        assert_eq!(rebuilt.current_name(), Some("View 1"));
    }
}
