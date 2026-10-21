//! 应用外壳：画布之外，重新构建的 egui 原生界面层。
//!
//! - [`chrome`]：菜单 / 工具条 / 左右停靠侧栏 / 状态栏；
//! - [`menubar`]：`gui/menu.xml` 生成的九棵菜单（构建期已校验动作 id）；
//! - [`theme`]：ArrowVortex 取向的深灰平面主题、紧凑尺寸与自绘控件；
//! - [`toolbars`]：动作图标缓存与工具条提示；
//! - [`dialogs`]：文件弹层、恢复、确认、偏好、关于等浮层；
//! - [`canvas`]：画布渲染与输入（`TiledDrawWidget`）；
//! - [`accelmap`] / [`actions`] / [`button_mapping`]：快捷键、动作与按键映射；
//! - [`overlays`]：画布内的辅助叠层；
//! - [`locales_generated`]：翻译表数据。
//!
//! 新 chrome 不恢复旧版 GTK 1:1 仿真皮肤：版式取 ArrowVortex 的紧凑、
//! 画布优先结构，颜色取 MyPaint 源 GTK / Adwaita Dark 的灰阶与选中蓝。
//! 顶栏/状态栏各一行，侧栏可拖宽、可独立收起，分隔一律 1px。

pub(crate) mod accelmap;
pub(crate) mod actions;
pub(crate) mod button_mapping;
pub(crate) mod canvas;
pub(crate) mod chrome;
pub(crate) mod dialogs;
pub(crate) mod locales_generated;
pub(crate) mod menubar;
pub(crate) mod overlays;
pub(crate) mod theme;
pub(crate) mod toolbars;
