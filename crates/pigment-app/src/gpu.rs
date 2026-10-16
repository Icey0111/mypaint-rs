//! GPU tile 渲染器：egui PaintCallback 内的自定义 wgpu render pass。
//!
//! - 每 tile 一张 64×64 `Rgba8UnormSrgb` 纹理（CPU 侧已完成 fix15→sRGB 合成）
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
use pigment_surface::PigmentSurface;

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
    surface: &PigmentSurface,
    uploaded_rev: impl Fn((i32, i32)) -> Option<u64>,
    visible: &[(i32, i32)],
    cap: usize,
) -> Vec<(i32, i32)> {
    let mut plan = Vec::new();
    for &coord in visible {
        if plan.len() >= cap {
            break;
        }
        let Some(tile) = surface.tile_entry(coord.0, coord.1) else {
            continue; // 空白 tile 不占用显存，由面板白底兜底
        };
        if uploaded_rev(coord) == Some(tile.revision()) {
            continue; // 已同步到位
        }
        plan.push(coord);
    }
    plan
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
    view_buf: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    tile_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    cache: HashMap<(i32, i32), GpuTile>,
    frame: u64,
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
                    blend: None, // tile 已在 CPU 侧合成到白纸底，输出不透明
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
            view_buf,
            view_bg,
            tile_layout,
            sampler,
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
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
    /// 画布矩形覆盖的可见 tile 坐标（可能含尚未有内容的空 tile）。
    pub visible: Vec<(i32, i32)>,
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

    /// 造一个含内容的 tile（写入一个像素即会 bump revision）。
    fn touch(surface: &mut PigmentSurface, tx: i32, ty: i32) {
        let t = surface.tile_request_start(tx, ty);
        t[0][0] = [100, 200, 300, 1 << 15];
        surface.tile_request_end(tx, ty);
    }

    #[test]
    fn plans_content_tiles_not_yet_uploaded() {
        let mut s = PigmentSurface::new();
        touch(&mut s, 0, 0);
        touch(&mut s, 1, 0);
        let visible = vec![(0, 0), (1, 0), (2, 0)]; // (2,0) 是空白 tile
        let plan = plan_uploads(&s, |_| None, &visible, 16);
        assert_eq!(plan, vec![(0, 0), (1, 0)], "空白 tile 不该占用上传配额");
    }

    #[test]
    fn skips_tiles_already_synced() {
        let mut s = PigmentSurface::new();
        touch(&mut s, 0, 0);
        touch(&mut s, 1, 0);
        let uploaded = |c: (i32, i32)| s.tile_entry(c.0, c.1).map(|t| t.revision());
        let visible = vec![(0, 0), (1, 0)];
        assert!(plan_uploads(&s, uploaded, &visible, 16).is_empty());
    }

    #[test]
    fn replans_after_cpu_change() {
        let mut s = PigmentSurface::new();
        touch(&mut s, 0, 0);
        touch(&mut s, 1, 0);
        let mut uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        for c in [(0, 0), (1, 0)] {
            uploaded.insert(c, s.tile_entry(c.0, c.1).unwrap().revision());
        }
        // 只改 (1,0)
        touch(&mut s, 1, 0);
        let plan = plan_uploads(&s, |c| uploaded.get(&c).copied(), &[(0, 0), (1, 0)], 16);
        assert_eq!(plan, vec![(1, 0)]);
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
        let mut s = PigmentSurface::new();
        touch(&mut s, 0, 0);
        touch(&mut s, 3, 2);
        let visible = vec![(0, 0), (3, 2)];
        // GPU 侧完全没有这些 tile（模拟第一趟 pass 被丢弃、什么都没上传）
        let uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        let probe = |c: (i32, i32)| uploaded.get(&c).copied();

        let pass1 = plan_uploads(&s, probe, &visible, 16);
        let pass2 = plan_uploads(&s, probe, &visible, 16);
        assert_eq!(pass1, pass2, "同一状态下重复规划必须得到相同结果");

        // 第二趟真正上传后，第三趟无需再做任何事（收敛）
        let mut synced = HashMap::new();
        for c in &pass1 {
            synced.insert(*c, s.tile_entry(c.0, c.1).unwrap().revision());
        }
        assert!(plan_uploads(&s, |c| synced.get(&c).copied(), &visible, 16).is_empty());
    }

    #[test]
    fn respects_per_frame_cap_without_losing_backlog() {
        let mut s = PigmentSurface::new();
        let mut visible = Vec::new();
        for tx in 0..10 {
            touch(&mut s, tx, 0);
            visible.push((tx, 0));
        }
        let uploaded: HashMap<(i32, i32), u64> = HashMap::new();
        let probe = |c: (i32, i32)| uploaded.get(&c).copied();
        // 每帧只放 4 个：本轮 4 个，欠账仍在，下轮继续（不会漏）
        let first = plan_uploads(&s, probe, &visible, 4);
        assert_eq!(first.len(), 4);
        assert_eq!(first, vec![(0, 0), (1, 0), (2, 0), (3, 0)]);
        let mut synced: HashMap<(i32, i32), u64> = HashMap::new();
        for c in &first {
            synced.insert(*c, s.tile_entry(c.0, c.1).unwrap().revision());
        }
        let second = plan_uploads(&s, |c| synced.get(&c).copied(), &visible, 4);
        assert_eq!(second, vec![(4, 0), (5, 0), (6, 0), (7, 0)]);
    }
}
