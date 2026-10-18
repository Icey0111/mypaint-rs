//! GPU tile 渲染器：egui PaintCallback 内的自定义 wgpu render pass。
//!
//! - 每 tile 一张 64×64 纹理（CPU 侧已完成 fix15→EOTF sRGB 合成）；
//!   纹理的色彩空间跟着渲染目标走，见 [`tile_format`] —— 这里对着一次就
//!   会让空白 tile 显示成暗方块，把每一笔变成一个个 tile 方格
//! - LRU 显存驻留：超出容量按 `last_frame` 淘汰最久未用者
//! - **自愈式增量同步**：每帧对可见 tile 比对「CPU revision vs GPU 已上传
//!   revision」，不一致才上传。每个 GpuTile 记住自己上传过的 revision，
//!   因此任何一帧的同步决策都是**幂等、可重算**的——egui 的多趟 UI pass
//!   （`request_discard` 重跑布局时丢弃前一趟形状）不会造成上传丢失。
//!   早期版本用「一次性消费的 changed_since 同步点 + 立即推进」，被丢弃的
//!   那一趟 pass 会永久丢失上传，画布留下笔画中途的半透明残影（已修）。
//! - 绘制：每个可见 tile 一次 draw call（6 顶点 quad，vertex_index 拉取，
//!   无顶点缓冲）；视口内 tile 数量有限（1080p ≈ 500 块），M2 阶段足够，
//!   instancing/atlas 合并留作 M5 优化项

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui;
use eframe::egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;

use crate::convert::TILE_RGBA8_BYTES;
use pigment_engine::hokusai::tile::TILE_SIZE;

/// 显存驻留上限（1024 块 × 16KiB = 16 MiB）。
const GPU_TILE_CAPACITY: usize = 1024;

/// 单帧上传上限（128 块 × 16KiB = 2 MiB）。超出的部分在后续帧继续补，
/// 因为同步决策每帧重算，**只延迟、不丢**。
pub const MAX_UPLOADS_PER_FRAME: usize = 128;

/// 一次 tile 上传请求（UI 线程收集，prepare 阶段消费）。
pub struct TileUpload {
    pub coord: (i32, i32),
    /// 本次上传对应的 CPU 侧 revision，写入后作为该 GPU tile 的「已同步水位」。
    pub revision: u64,
    pub pixels: Box<[u8; TILE_RGBA8_BYTES]>,
}

/// 计算本帧需要上传的可见 tile 坐标。
///
/// 判据是**状态比较**而非事件流水：只要 GPU 记住的已同步 revision 与 CPU 当前
/// revision 不同（含 GPU 根本没有该 tile、或已被 LRU 淘汰），就需要（重）上传。
/// 纯函数 + 幂等：同一状态下重复调用结果一致，因此 UI pass 被丢弃重跑也不会丢同步。
pub fn plan_uploads(
    uploaded_rev: impl Fn((i32, i32)) -> Option<u64>,
    visible: &[(i32, i32)],
    cap: usize,
    has_content: impl Fn((i32, i32)) -> bool,
    composite_rev: u64,
) -> Vec<(i32, i32)> {
    let mut plan = Vec::new();
    for &coord in visible {
        if plan.len() >= cap {
            break;
        }
        if !has_content(coord) {
            continue; // 空白 tile 不占用显存，由面板底色兜底
        }
        if uploaded_rev(coord) == Some(composite_rev) {
            continue; // 已同步到位
        }
        plan.push(coord);
    }
    plan
}

/// 计算本帧需要从显存**剔除**的可见 tile：CPU 已经没有它，显存却还留着。
///
/// 撤销/重做会把表面退回较早的快照（`PigmentSurface::load_snapshot`），那时
/// 画布上"多出来"的 tile 会从 CPU 侧消失；`plan_uploads` 对缺席的坐标只会
/// 跳过（空白 tile 不占显存），于是显存里的旧纹理继续被画出来——表现就是
/// "撤销不掉"。所以这里把这类坐标显式列出来交给渲染层删除。
///
/// 与 `plan_uploads` 一样是纯函数 + 幂等：同一状态下重复调用结果一致，
/// UI pass 被丢弃重跑也不会误删或漏删。
pub fn plan_evictions(
    resident: impl Fn((i32, i32)) -> bool,
    visible: &[(i32, i32)],
    has_content: impl Fn((i32, i32)) -> bool,
) -> Vec<(i32, i32)> {
    visible
        .iter()
        .copied()
        .filter(|c| !has_content(*c) && resident(*c))
        .collect()
}

struct GpuTile {
    // wgpu 对象内部引用计数，bind_group 持有即保活，无需再存 texture/buffer 句柄
    bind_group: wgpu::BindGroup,
    texture: wgpu::Texture, // 保留句柄：覆盖写时 write_texture 需要
    /// 已同步到显存的 CPU revision（自愈式同步的水位线）。
    uploaded_rev: u64,
    last_frame: u64,
}

pub struct TileRenderer {
    pipeline: wgpu::RenderPipeline,
    /// 透明棋盘格背景（全屏三角形，无绑定组）。
    check_pipeline: wgpu::RenderPipeline,
    view_buf: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// tile 纹理的颜色空间 —— 必须与渲染目标匹配，见 [`TileRenderer::tile_format`]。
    tile_format: wgpu::TextureFormat,
    cache: HashMap<(i32, i32), GpuTile>,
    frame: u64,
}

/// 依渲染目标选 tile 纹理格式。
///
/// CPU 侧 [`crate::convert::tile_to_rgba8`] 交出来的是**显示编码值**
/// （EOTF 2.2 之后的 8 位），不是线性值。所以纹理格式必须和渲染目标的色彩空间
/// 一致，否则会多跑一次转换：
/// - 目标带 sRGB → 纹理也用 sRGB：采样时硬件解码、写出时硬件编码，一来一回抵消；
/// - 目标不带 sRGB（egui 的 `target_format` 就可能是不带 sRGB 的 `*Unorm`）→ 纹理
///   也得用 `*Unorm`，否则采样时被解码一次而写出时不编码，画面**整体压暗**。
///
/// 后者正是本项目曾经的缺陷：空白 tile 本应与画布底色完全同色，实际却显示成
/// `237^(2.2) ≈ 216` 的暗方块，让每一笔都露出一个个 tile 方格。
fn tile_format(target: wgpu::TextureFormat) -> wgpu::TextureFormat {
    if target.is_srgb() {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

const SHADER: &str = r#"
struct ViewUniform { m: mat4x4<f32>, };
@group(0) @binding(0) var<uniform> view: ViewUniform;

struct TileUniform { ofs: vec4<f32>, };
@group(1) @binding(0) var tile_tex: texture_2d<f32>;
@group(1) @binding(1) var tile_samp: sampler;
@group(1) @binding(2) var<uniform> tile: TileUniform;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(64.0, 0.0), vec2<f32>(0.0, 64.0),
        vec2<f32>(0.0, 64.0), vec2<f32>(64.0, 0.0), vec2<f32>(64.0, 64.0)
    );
    let c = corners[vi];
    var out: VsOut;
    out.pos = view.m * vec4<f32>(c + tile.ofs.xy, 0.0, 1.0);
    out.uv = c / 64.0;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(tile_tex, tile_samp, in.uv);
}

@vertex
fn vs_check(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0)
    );
    return vec4<f32>(p[vi], 0.0, 1.0);
}

@fragment
fn fs_check(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // 16×16 屏幕像素、两种灰（style.py:24-26）
    let cell = floor(pos.xy / 16.0);
    let odd = (cell.x + cell.y) - 2.0 * floor((cell.x + cell.y) / 2.0);
    let c = mix(0.5, 0.45, odd);
    return vec4<f32>(c, c, c, 1.0);
}
"#;

fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = vec![0u8; v.len() * 4];
    for (i, f) in v.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&f.to_ne_bytes());
    }
    out
}

impl TileRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pigment tile shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let view_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view uniform"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("view bg"),
            layout: &view_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buf.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("tile sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pigment pipeline layout"),
            bind_group_layouts: &[Some(&view_layout), Some(&tile_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pigment tile pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    // 背景不可见时 tile 保留**预乘** alpha（棋盘格透出）
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // 透明棋盘格：无绑定组、不混合，画在 tile 之前
        let check_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("alpha check layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let check_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("alpha check pipeline"),
            layout: Some(&check_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_check"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_check"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            check_pipeline,
            view_buf,
            view_bg,
            tile_layout,
            sampler,
            tile_format: tile_format(target_format),
            cache: HashMap::new(),
            frame: 0,
        }
    }

    fn set_view(&self, queue: &wgpu::Queue, m: &[[f32; 4]; 4]) {
        let flat: Vec<f32> = m.iter().flatten().copied().collect();
        queue.write_buffer(&self.view_buf, 0, &f32s_to_bytes(&flat));
    }

    /// 上传/覆盖一块 tile 纹理。
    fn upload_tile(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        up: &TileUpload,
    ) {
        let size = wgpu::Extent3d {
            width: TILE_SIZE as u32,
            height: TILE_SIZE as u32,
            depth_or_array_layers: 1,
        };
        let frame = self.frame;
        if let Some(t) = self.cache.get_mut(&up.coord) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &t.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &up.pixels[..],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((TILE_SIZE * 4) as u32),
                    rows_per_image: Some(TILE_SIZE as u32),
                },
                size,
            );
            t.last_frame = frame;
            t.uploaded_rev = up.revision;
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.tile_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &up.pixels[..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((TILE_SIZE * 4) as u32),
                rows_per_image: Some(TILE_SIZE as u32),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let ofs = [
            (up.coord.0 * TILE_SIZE as i32) as f32,
            (up.coord.1 * TILE_SIZE as i32) as f32,
            0.0,
            0.0,
        ];
        let ofs_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile ofs"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ofs_buf, 0, &f32s_to_bytes(&ofs));

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tile bg"),
            layout: &self.tile_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ofs_buf.as_entire_binding(),
                },
            ],
        });

        self.cache.insert(
            up.coord,
            GpuTile {
                texture,
                bind_group,
                uploaded_rev: up.revision,
                last_frame: frame,
            },
        );
    }

    /// 该 tile 已同步到显存的 CPU revision（`None` = 显存无此 tile）。
    #[inline]
    pub fn uploaded_rev(&self, coord: (i32, i32)) -> Option<u64> {
        self.cache.get(&coord).map(|t| t.uploaded_rev)
    }

    /// 当前显存驻留的 tile 数。
    #[inline]
    pub fn resident(&self) -> usize {
        self.cache.len()
    }

    /// 清空显存缓存（下一次同步会按需重建全部可见 tile）。
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
    }

    /// 剔除单个 tile 的显存副本（撤销后画布上已不存在的内容）。
    #[inline]
    pub fn evict(&mut self, coord: (i32, i32)) {
        self.cache.remove(&coord);
    }

    /// LRU 淘汰：超出容量时移除最久未用的 tile。
    fn evict_lru(&mut self) {
        if self.cache.len() <= GPU_TILE_CAPACITY {
            return;
        }
        let excess = self.cache.len() - GPU_TILE_CAPACITY;
        let mut by_frame: Vec<((i32, i32), u64)> =
            self.cache.iter().map(|(&c, t)| (c, t.last_frame)).collect();
        by_frame.sort_by_key(|&(_, f)| f);
        for (coord, _) in by_frame.into_iter().take(excess) {
            self.cache.remove(&coord);
        }
    }
}

/// egui PaintCallback：每帧携带待上传 tile 包 + 可见 tile 列表 + 视图矩阵。
pub struct CanvasPaint {
    pub renderer: Arc<Mutex<TileRenderer>>,
    /// 世界像素 → 全窗口裁剪空间矩阵（窗口物理像素域）。
    pub view_matrix: [[f32; 4]; 4],
    /// 本帧需要上传的脏 tile。
    pub uploads: Vec<TileUpload>,
    /// 本帧需要从显存剔除的 tile（撤销后已不在画布上）。
    pub evict: Vec<(i32, i32)>,
    /// 画布矩形覆盖的可见 tile 坐标（可能含尚未有内容的空 tile）。
    pub visible: Vec<(i32, i32)>,
    /// 是否先画透明棋盘格（背景不可见 + `view.real_alpha_checks`）。
    pub checks: bool,
}

impl CallbackTrait for CanvasPaint {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut r = self.renderer.lock().unwrap();
        r.frame += 1;
        for up in &self.uploads {
            r.upload_tile(device, queue, up);
        }
        for coord in &self.evict {
            r.evict(*coord);
        }
        r.set_view(queue, &self.view_matrix);
        r.evict_lru();
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        _resources: &CallbackResources,
    ) {
        let r = self.renderer.lock().unwrap();
        if self.checks {
            render_pass.set_pipeline(&r.check_pipeline);
            render_pass.draw(0..3, 0..1);
        }
        render_pass.set_pipeline(&r.pipeline);
        render_pass.set_bind_group(0, &r.view_bg, &[]);
        for coord in &self.visible {
            let Some(t) = r.cache.get(coord) else { continue };
            render_pass.set_bind_group(1, &t.bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigment_engine::hokusai::TiledSurface;
    use pigment_surface::{LayerStack, PigmentSurface};

    /// 造一个含内容的 tile（写入一个像素即会 bump revision）。
    fn touch(surface: &mut PigmentSurface, tx: i32, ty: i32) {
        let t = surface.tile_request_start(tx, ty);
        t[0][0] = [100, 200, 300, 1 << 15];
        surface.tile_request_end(tx, ty);
    }

    #[test]
    fn tile_format_follows_target_colorspace() {
        // 目标带 sRGB → 纹理也带：采样解码、写出编码，一来一回抵消
        assert_eq!(
            tile_format(wgpu::TextureFormat::Bgra8UnormSrgb),
            wgpu::TextureFormat::Rgba8UnormSrgb
        );
        // 目标不带 sRGB → 纹理也不能带：CPU 给的是显示编码值，多解码一次
        // 就会把空白 tile 压成 `237^(2.2)≈216` 的暗方块（曾经的实际缺陷）
        assert_eq!(
            tile_format(wgpu::TextureFormat::Bgra8Unorm),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }

    #[test]
    fn plans_content_tiles_not_yet_uploaded() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let visible = vec![(0, 0), (1, 0), (2, 0)]; // (2,0) 是空白 tile
        let rev = st.composite_revision();
        let plan = plan_uploads(
            |_| None,
            &visible,
            16,
            |c| st.has_paint_at(c.0, c.1),
            rev,
        );
        assert_eq!(plan, vec![(0, 0), (1, 0)], "空白 tile 不该占用上传配额");
    }

    #[test]
    fn skips_tiles_already_synced() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let rev = st.composite_revision();
        let uploaded = |_c: (i32, i32)| Some(rev);
        let visible = vec![(0, 0), (1, 0)];
        assert!(plan_uploads(uploaded, &visible, 16, |c| st.has_paint_at(c.0, c.1), rev).is_empty());
    }

    #[test]
    fn replans_after_cpu_change() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let mut uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        for c in [(0, 0), (1, 0)] {
            uploaded.insert(c, st.composite_revision());
        }
        // 只改 (1,0)：全局合成版本号被抬高。
        // 粒度权衡（记账为后续逐 tile 哈希优化）：两个 tile 都会重传，
        // (0,0) 重传的是相同内容——正确性不受影响，代价是少量冗余带宽。
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let rev = st.composite_revision();
        let plan = plan_uploads(
            |c| uploaded.get(&c).copied(),
            &[(0, 0), (1, 0)],
            16,
            |c| st.has_paint_at(c.0, c.1),
            rev,
        );
        assert_eq!(plan, vec![(0, 0), (1, 0)]);
    }

    /// 关键回归：同步决策必须**幂等**。
    ///
    /// 旧实现用「changed_since + 立即推进 last_sync」做事件流水式同步，egui 多趟
    /// UI pass 中若第一趟的 PaintCallback 被丢弃（request_discard 重跑布局），
    /// 那批上传就永久丢失，tile 会停在笔画中途的半透明快照上——表现为画面
    /// 「缺了一角 / 像被空白挡住」。新实现基于状态比较，重复规划结果一致，
    /// 因此某一趟 pass 被丢弃不影响最终收敛。
    #[test]
    fn planning_is_idempotent_across_discarded_passes() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 3, 2);
        let visible = vec![(0, 0), (3, 2)];
        let rev = st.composite_revision();
        // GPU 侧完全没有这些 tile（模拟第一趟 pass 被丢弃、什么都没上传）
        let uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        let probe = |c: (i32, i32)| uploaded.get(&c).copied();

        let pass1 = plan_uploads(probe, &visible, 16, |c| st.has_paint_at(c.0, c.1), rev);
        let pass2 = plan_uploads(probe, &visible, 16, |c| st.has_paint_at(c.0, c.1), rev);
        assert_eq!(pass1, pass2, "同一状态下重复规划必须得到相同结果");

        // 第二趟真正上传后，第三趟无需再做任何事（收敛）
        let mut synced = HashMap::new();
        for c in &pass1 {
            synced.insert(*c, rev);
        }
        assert!(
            plan_uploads(|c| synced.get(&c).copied(), &visible, 16, |c| st.has_paint_at(c.0, c.1), rev)
                .is_empty()
        );
    }

    #[test]
    fn respects_per_frame_cap_without_losing_backlog() {
        let mut st = LayerStack::new();
        let mut visible = Vec::new();
        for tx in 0..10 {
            touch(&mut st.current_mut().surface_mut().unwrap(), tx, 0);
            visible.push((tx, 0));
        }
        let rev = st.composite_revision();
        let uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        let probe = |c: (i32, i32)| uploaded.get(&c).copied();
        let has = |c: (i32, i32)| st.has_content_at(c.0, c.1);
        // 每帧只放 4 个：本轮 4 个，欠账仍在，下轮继续（不会漏）
        let first = plan_uploads(probe, &visible, 4, has, rev);
        assert_eq!(first.len(), 4);
        assert_eq!(first, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        let mut synced: HashMap<(i32, i32), u64> = HashMap::new();
        for c in &first {
            synced.insert(*c, rev);
        }
        let second = plan_uploads(|c| synced.get(&c).copied(), &visible, 4, has, rev);
        assert_eq!(second, vec![(4, 0), (5, 0), (6, 0), (7, 0)]);
    }

    // ---- 撤销/重做引发的显存剔除 ----

    /// 撤销后画布上"多出来"的 tile 必须从显存删掉。
    ///
    /// 复现路径：画两笔 → 显存里有 (0,0)(1,0) → 撤销掉第二笔 → CPU 只剩 (0,0)，
    /// 但 (1,0) 的纹理还在显存里被画出来。这就是"撤销不掉"的成因。
    #[test]
    fn plans_eviction_for_tiles_removed_by_undo() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let sshot = st.current().surface().unwrap().save_snapshot();
        touch(&mut st.current_mut().surface_mut().unwrap(), 2, 0);
        let _ = st.current_mut().surface_mut().unwrap().load_snapshot(&sshot); // 撤销：只剩 (0,0)(1,0)

        let visible = vec![(0, 0), (1, 0), (2, 0)];
        // 显存里三块都在（撤销前上传过）
        let resident = |c: (i32, i32)| matches!(c, (0, 0) | (1, 0) | (2, 0));
        assert_eq!(
            plan_evictions(resident, &visible, |c| st.has_paint_at(c.0, c.1)),
            vec![(2, 0)],
            "(2,0) 已被撤销掉但显存里还在 → 必须剔除"
        );
    }

    /// CPU 有内容、显存已同步的 tile 不该被剔除（正常作画路径不受影响）。
    #[test]
    fn does_not_evict_tiles_still_present_on_cpu() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let visible = vec![(0, 0), (1, 0)];
        assert!(
            plan_evictions(|_| true, &visible, |c| st.has_paint_at(c.0, c.1)).is_empty(),
            "CPU 仍在的 tile 不该被剔除"
        );
    }

    /// 显存里本来就没有的空白 tile 不必下发删除指令。
    #[test]
    fn does_not_evict_when_not_resident() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        let visible = vec![(0, 0), (7, 7)];
        assert!(
            plan_evictions(|_| false, &visible, |c| st.has_paint_at(c.0, c.1)).is_empty(),
            "显存里没有的东西无需剔除"
        );
    }

    /// 视口外的残留 tile 不处理：它们既不显示，也会被 LRU 自然淘汰。
    #[test]
    fn eviction_is_limited_to_the_visible_set() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        let visible = vec![(0, 0)]; // (9,9) 曾被上传但现在不在视口里
        assert!(plan_evictions(|c| c == (9, 9), &visible, |c| st.has_paint_at(c.0, c.1)).is_empty());
    }

    /// 与上传规划一样是纯函数：同一状态下重复规划结果一致
    /// （egui 多趟 pass 丢弃重跑时不会误删画布内容）。
    #[test]
    fn eviction_planning_is_idempotent() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        let visible = vec![(0, 0), (1, 0)];
        let resident = |c: (i32, i32)| c == (1, 0);
        let a = plan_evictions(resident, &visible, |c| st.has_paint_at(c.0, c.1));
        let b = plan_evictions(resident, &visible, |c| st.has_paint_at(c.0, c.1));
        assert_eq!(a, b);
        assert_eq!(a, vec![(1, 0)]);
    }
}
