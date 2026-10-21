//! 图标缓存与工具条数据。
//!
//! MyPaint 的工具条图标是 GTK symbolic SVG，构建期已经由 `build.rs` 栅格化
//! 成 24×24 PNG（`$OUT_DIR/icons_generated.rs`）。这里不再复刻 GTK 的
//! `GtkToolbar`，只保留两件事：
//!
//! 1. 按动作的 `icon-name` 找图标、解码、着色、缓存纹理；
//! 2. 从动作表取标题/快捷键，生成工具条与菜单共用的提示文字。
//!
//! 视觉完全交给 [`super::theme::icon_button`]，本文件不画皮肤。

use std::collections::HashMap;

use eframe::egui;

use super::theme;

pub(crate) mod generated {
    include!(concat!(env!("OUT_DIR"), "/icons_generated.rs"));
}

use generated::ICONS;

/// 图标纹理缓存。键是解析后的实际图标名（可能带 `-symbolic` 后缀）。
#[derive(Default)]
pub(crate) struct IconCache {
    map: HashMap<String, egui::TextureHandle>,
}

impl IconCache {
    pub(crate) fn get(&self, name: &str) -> Option<&egui::TextureHandle> {
        self.map.get(name)
    }
}

/// 按图标名取纹理；找不到时补 `-symbolic` 再试一次。
///
/// 纹理统一着色为白色，绘制时再用 `tint` 上色 —— 这样同一个图标可以同时
/// 出现在暗色工具条、强调色当前工具和禁用态里，而不用为每种状态各存一份。
pub(crate) fn icon_texture(
    ctx: &egui::Context,
    cache: &mut IconCache,
    name: &str,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.map.get(name) {
        return Some(t.clone());
    }
    let (resolved, bytes) = if let Some(bytes) = lookup(name) {
        (name.to_string(), bytes)
    } else {
        let alt = format!("{name}-symbolic");
        (alt.clone(), lookup(&alt)?)
    };
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels: Vec<egui::Color32> = img
        .pixels()
        .map(|p| {
            let a = p.0[3];
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, a)
        })
        .collect();
    let tex = ctx.load_texture(
        format!("pigment-icon:{resolved}"),
        egui::ColorImage {
            size: [w, h],
            source_size: egui::Vec2::new(w as f32, h as f32),
            pixels,
        },
        egui::TextureOptions::LINEAR,
    );
    cache.map.insert(resolved, tex.clone());
    Some(tex)
}

fn lookup(name: &str) -> Option<&'static [u8]> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// 按动作 id 取图标名（`resources.xml` 的 `icon-name`）。
pub(crate) fn action_icon_name(id: &str) -> Option<&'static str> {
    crate::shell::actions::by_id(id)?.icon_name
}

/// 动作的短标题（工具条 tooltip 第一行）。
pub(crate) fn action_short_label(id: &str) -> String {
    crate::shell::actions::by_id(id)
        .and_then(|a| a.toolbar_label().or_else(|| a.effective_label()))
        .unwrap_or(id)
        .to_string()
}

/// 工具条按钮的 tooltip：`短标题` + 动作全名 + 快捷键。
pub(crate) fn action_tip(id: &str) -> String {
    let Some(a) = crate::shell::actions::by_id(id) else {
        return id.to_string();
    };
    let title = a
        .toolbar_label()
        .or_else(|| a.effective_label())
        .unwrap_or(id);
    let long = a.effective_label().unwrap_or(title);
    let mut tip = title.to_string();
    if long != title {
        tip.push('\n');
        tip.push_str(long);
    }
    if let Some(acc) = a.accel {
        tip.push_str("\n");
        tip.push_str(&acc.label());
    }
    if let Some(t) = a.effective_tooltip() {
        tip.push('\n');
        tip.push_str(t);
    }
    crate::i18n::tr(&tip).to_string()
}

/// 画一个动作按钮。返回点击响应（禁用时 `clicked()` 恒为 false）。
///
/// 图标缺失时退回动作短标题的第一个字符 —— 自绘按钮没有 `Button::new`
/// 那种自动文本回退，这里显式补上。
pub(crate) fn action_button(
    ui: &mut egui::Ui,
    cache: &mut IconCache,
    id: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let tex = action_icon_name(id).and_then(|n| icon_texture(ui.ctx(), cache, n));
    let fallback = action_short_label(id)
        .chars()
        .next()
        .unwrap_or('?')
        .to_string();
    let tip = if enabled {
        action_tip(id)
    } else {
        format!("{} · 当前不可用", action_short_label(id))
    };
    theme::icon_button(ui, tex.as_ref(), &fallback, selected, enabled, &tip)
}

/// 小号动作按钮（面板标题栏用）。
pub(crate) fn small_action_button(
    ui: &mut egui::Ui,
    cache: &mut IconCache,
    id: &str,
    enabled: bool,
) -> egui::Response {
    let tex = action_icon_name(id).and_then(|n| icon_texture(ui.ctx(), cache, n));
    let fallback = action_short_label(id)
        .chars()
        .next()
        .unwrap_or('?')
        .to_string();
    theme::small_icon_button(ui, tex.as_ref(), &fallback, enabled, &action_tip(id))
}
