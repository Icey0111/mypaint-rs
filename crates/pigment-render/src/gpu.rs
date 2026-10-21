//! GPU tile 渲染器：atlas 页 + instanced quad 的独立 wgpu 管线。
//!
//! - CPU 侧已完成 fix15→EOTF 显示编码合成，这里只负责把 RGBA8 tile 放进
//!   显存并在任意渲染目标上按视图矩阵铺出。
//! - **mipmap**：纹理永远是 64×64，但 level L 的一格覆盖 `64<<level` 世界
//!   像素（instance 的 scale = 2^L）。级别进 [`TileKey`]，远景与近景缓存
//!   互不污染。
//! - **atlas + instancing**：每块 tile 占 2048² atlas 页里的 64×64 格；
//!   一帧把所有可见格按页分组，一页一次 `draw(6, N)`。低缩放视口可能有
//!   500–2000 块可见格，旧实现是同样数量的 draw call + bind group 切换，
//!   GUI 线程经常因此卡在驱动提交上。
//! - LRU 显存驻留：容量随可见集合上浮，可见格是**白名单**，永不淘汰
//!   （见 [`plan_lru_evictions`]）。
//! - **自愈式增量同步**：每帧比对 CPU revision 与 GPU 已上传 revision，
//!   不一致才上传。GpuTile 记住上传水位线，所以同步决策幂等、可重算；
//!   多趟 UI pass 丢弃重跑不会丢上传。
//!
//! 本模块没有任何 egui 类型：GUI 只构造 [`TileUpload`]、传入视图矩阵与
//! draw 列表，然后调用 [`TileRenderer::prepare`] / [`TileRenderer::draw_prepared`]。

use std::collections::HashMap;

use hokusai::tile::TILE_SIZE;
use pigment_surface::color::TILE_RGBA8_BYTES;

/// 显存驻留的**下限**（1024 块 × 16KiB = 16 MiB）。
const GPU_TILE_CAPACITY: usize = 1024;

/// 容量随可见集合上浮的余量（"下几帧可能要上传"的排队空间）。
const GPU_TILE_SLACK: usize = 256;

/// 本帧允许占用的显存格数：至少 [`GPU_TILE_CAPACITY`]，且必须装得下全部
/// 可见格 + [`GPU_TILE_SLACK`]。
#[inline]
pub fn capacity_for(visible: usize) -> usize {
    GPU_TILE_CAPACITY.max(visible + GPU_TILE_SLACK)
}

/// 纯函数：LRU 淘汰计划 —— 从缓存条目里挑出要丢弃的 key。
///
/// **可见格一律不淘汰**（`visible` 是白名单）：一旦允许淘汰可见格，就会
/// 出现「上传 → 立刻被自己挤掉 → 下一帧再上传」的活锁。
pub fn plan_lru_evictions(
    entries: &[(TileKey, u64)],
    visible: &[TileKey],
    capacity: usize,
) -> Vec<TileKey> {
    if entries.len() <= capacity {
        return Vec::new();
    }
    let excess = entries.len() - capacity;
    let protect: std::collections::HashSet<TileKey> = visible.iter().copied().collect();
    let mut stale: Vec<(TileKey, u64)> = entries
        .iter()
        .copied()
        .filter(|(k, _)| !protect.contains(k))
        .collect();
    stale.sort_by_key(|&(_, f)| f);
    stale.into_iter().take(excess).map(|(k, _)| k).collect()
}

/// 一次 tile 上传请求（UI 线程收集，prepare 阶段消费）。
pub struct TileUpload {
    pub key: TileKey,
    /// 本次上传对应的 CPU 侧 revision，写入后作为该 GPU tile 的「已同步水位」。
    pub revision: u64,
    pub pixels: Box<[u8; TILE_RGBA8_BYTES]>,
}

/// 本帧要画的一块 tile。
///
/// `key` 决定世界位置与屏幕尺寸；`source` 是实际取样的显存 tile。
/// 二者通常相同。缩放切换、目标级别还没上传完时，会用相邻级别已驻留的
/// tile 回退：`source` 取粗一级的祖先格，`sub` 指定该祖先里的哪一块
/// 子矩形（64×64 texel 坐标），这样新旧级别**不会重叠绘制**，也就不会
/// 出现双重混合或空洞。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawTile {
    /// 世界位置 / 放置级别。
    pub key: TileKey,
    /// 显存里的取样 tile（可以来自别的 mip 级别）。
    pub source: TileKey,
    /// 在 `source` 的 64×64 texel 内的子矩形 `[x0, y0, x1, y1]`。
    pub sub: [u8; 4],
}

impl DrawTile {
    /// 直接画本级 tile。
    #[inline]
    pub fn normal(key: TileKey) -> Self {
        Self {
            key,
            source: key,
            sub: [0, 0, TILE_SIZE as u8, TILE_SIZE as u8],
        }
    }
}

/// 显存 tile 的键：**级别 + 级别空间坐标**。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileKey {
    pub level: u32,
    pub tx: i32,
    pub ty: i32,
}

impl TileKey {
    #[inline]
    pub fn new(level: u32, tx: i32, ty: i32) -> Self {
        Self { level, tx, ty }
    }

    /// 该 tile 覆盖的世界边长（像素）：`64 << level`。
    #[inline]
    pub fn world_size(self) -> i32 {
        TILE_SIZE as i32 * (1i32 << self.level)
    }

    /// 该 tile 左上角的世界像素坐标（`tx * world_size`）。
    #[inline]
    pub fn world_origin(self) -> (i32, i32) {
        let s = self.world_size();
        (self.tx * s, self.ty * s)
    }
}

/// 计算本帧需要上传的可见 tile（纯函数、幂等）。
pub fn plan_uploads(
    uploaded_rev: impl Fn(TileKey) -> Option<u64>,
    visible: &[TileKey],
    cap: usize,
    has_content: impl Fn(TileKey) -> bool,
    rev_of: impl Fn(TileKey) -> u64,
) -> Vec<TileKey> {
    let mut plan = Vec::new();
    for &key in visible {
        if plan.len() >= cap {
            break;
        }
        if !has_content(key) {
            continue;
        }
        if uploaded_rev(key) == Some(rev_of(key)) {
            continue;
        }
        plan.push(key);
    }
    plan
}

/// 计算本帧需要从显存**剔除**的可见 tile：CPU 已经没有它，显存却还留着。
pub fn plan_evictions(
    resident: impl Fn(TileKey) -> bool,
    visible: &[TileKey],
    has_content: impl Fn(TileKey) -> bool,
) -> Vec<TileKey> {
    visible
        .iter()
        .copied()
        .filter(|k| !has_content(*k) && resident(*k))
        .collect()
}

/// 依渲染目标选 tile 纹理格式。
///
/// CPU 侧交出来的是**显示编码值**（EOTF 2.2 之后的 8 位），不是线性值：
/// 目标带 sRGB → 纹理也用 sRGB；目标不带 sRGB → 纹理也用 Unorm。
pub fn tile_format(target: wgpu::TextureFormat) -> wgpu::TextureFormat {
    if target.is_srgb() {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    }
}

/// atlas 页边长（像素）。2048² RGBA8 = 16 MiB，可放 32×32 = 1024 块 64² tile。
const ATLAS_PAGE_SIZE: u32 = 2048;
/// 每行/每列的 tile 槽数。
const ATLAS_SLOTS_PER_ROW: u32 = ATLAS_PAGE_SIZE / TILE_SIZE as u32;
/// 每页槽数。
const ATLAS_SLOTS_PER_PAGE: u32 = ATLAS_SLOTS_PER_ROW * ATLAS_SLOTS_PER_ROW;

/// 每个 instance 8 个 f32（32 字节）：origin.xy, scale, uv_min.xy, uv_max.xy, pad。
const INSTANCE_STRIDE: u64 = 32;

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: 8,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 12,
        shader_location: 2,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 20,
        shader_location: 3,
    },
];

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Instance {
    origin: [f32; 2],
    scale: f32,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    _pad: f32,
}

/// 一页 atlas 的 GPU 资源与空闲槽位。
struct AtlasPage {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    free_slots: Vec<u32>,
}

struct GpuTile {
    page: u32,
    slot: u32,
    uploaded_rev: u64,
    last_frame: u64,
}

/// 按 atlas 页分组的一次 draw。
#[derive(Clone, Copy, Debug)]
struct DrawBatch {
    page: u32,
    first: u32,
    count: u32,
}

pub struct TileRenderer {
    pipeline: wgpu::RenderPipeline,
    /// 透明棋盘格背景（全屏三角形，无绑定组）。
    check_pipeline: wgpu::RenderPipeline,
    view_buf: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    page_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// tile 纹理的颜色空间 —— 必须与渲染目标匹配。
    tile_format: wgpu::TextureFormat,
    pages: Vec<AtlasPage>,
    cache: HashMap<TileKey, GpuTile>,
    instance_buf: Option<wgpu::Buffer>,
    instance_capacity: usize,
    batches: Vec<DrawBatch>,
    frame: u64,
}

const SHADER: &str = r#"
struct ViewUniform {
    m: mat4x4<f32>,
    // x > 0.5 → 最近邻（缩放 > 2.8×，与 MyPaint 的 pixelize_threshold 一致）。
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> view: ViewUniform;

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
// 采样器保留在绑定布局里但不再使用：改为逐 texel 手动取样，
// 这样才能跨 tile 边界无缝。
@group(1) @binding(1) var atlas_samp: sampler;

const PAGE: f32 = 2048.0;
const TS: f32 = 64.0;

struct Inst {
    @location(0) origin: vec2<f32>,
    @location(1) scale: f32,
    @location(2) uv_min: vec2<f32>,
    @location(3) uv_max: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // 0..1 的 tile 内坐标（不是 UV）。
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) uv_min: vec2<f32>,
    @location(2) @interpolate(flat) span: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(64.0, 0.0), vec2<f32>(0.0, 64.0),
        vec2<f32>(0.0, 64.0), vec2<f32>(64.0, 0.0), vec2<f32>(64.0, 64.0)
    );
    let c = corners[vi];
    // origin = tile 左上角世界像素；scale = 每个 texel 覆盖的世界像素数
    //（= 2^mipmap_level）。quad 按 scale 放大。
    var out: VsOut;
    out.pos = view.m * vec4<f32>(inst.origin + c * inst.scale, 0.0, 1.0);
    out.local = c / 64.0;
    out.uv_min = inst.uv_min;
    out.span = (inst.uv_max.x - inst.uv_min.x) * PAGE;
    return out;
}

// 从 atlas 读一个 texel；i 是相对 sub-rect 原点的 texel 坐标，
// 夹到该 tile 的 64×64 槽内（绝不读进相邻槽），边界外复制边缘 texel。
fn load_texel(slot: vec2<f32>, i: vec2<i32>) -> vec4<f32> {
    let c = vec2<i32>(slot) + clamp(i, vec2<i32>(0), vec2<i32>(63));
    return textureLoad(atlas_tex, c, 0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let slot = floor(in.uv_min * PAGE / TS) * TS;
    let sub = in.uv_min * PAGE - slot;
    // texel 中心在整数坐标：t = texel 坐标 - 0.5。
    let t = sub + in.local * in.span - vec2<f32>(0.5);
    if view.params.x > 0.5 {
        let ip = vec2<i32>(floor(t + vec2<f32>(0.5)));
        return load_texel(slot, ip);
    }
    let base = vec2<i32>(floor(t));
    let f = t - vec2<f32>(floor(t));
    let c00 = load_texel(slot, base);
    let c10 = load_texel(slot, base + vec2<i32>(1, 0));
    let c01 = load_texel(slot, base + vec2<i32>(0, 1));
    let c11 = load_texel(slot, base + vec2<i32>(1, 1));
    return mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
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

fn slot_xy(slot: u32) -> (u32, u32) {
    (slot % ATLAS_SLOTS_PER_ROW, slot / ATLAS_SLOTS_PER_ROW)
}

/// 槽位在 atlas 里的 UV 矩形（**完整** 64 texel，不做内缩）。
///
/// 着色器改为逐 texel 手动取样：内缩会把 64 个世界像素压进 63 个 texel，
/// 每格内容偏移 1.6%，高缩放下就是可见的接缝/错位。完整槽位 + 手动夹取
/// 既能精确对齐，又不会读到相邻槽。
fn slot_uv(slot: u32) -> [f32; 4] {
    let (sx, sy) = slot_xy(slot);
    let size = ATLAS_PAGE_SIZE as f32;
    let x0 = (sx * TILE_SIZE as u32) as f32;
    let y0 = (sy * TILE_SIZE as u32) as f32;
    let edge = TILE_SIZE as f32;
    [x0 / size, y0 / size, (x0 + edge) / size, (y0 + edge) / size]
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
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let page_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas page layout"),
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
            ],
        });

        let view_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("view uniform"),
            size: 80,
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
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pigment pipeline layout"),
            bind_group_layouts: &[Some(&view_layout), Some(&page_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pigment tile pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                })],
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
            page_layout,
            sampler,
            tile_format: tile_format(target_format),
            pages: Vec::new(),
            cache: HashMap::new(),
            instance_buf: None,
            instance_capacity: 0,
            batches: Vec::new(),
            frame: 0,
        }
    }

    fn set_view(&self, queue: &wgpu::Queue, m: &[[f32; 4]; 4], nearest: bool) {
        let mut flat: Vec<f32> = m.iter().flatten().copied().collect();
        flat.extend_from_slice(&[if nearest { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]);
        queue.write_buffer(&self.view_buf, 0, &f32s_to_bytes(&flat));
    }

    /// 新建一页 atlas（含 bind group），返回页索引。
    fn create_page(&mut self, device: &wgpu::Device) -> u32 {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tile atlas page"),
            size: wgpu::Extent3d {
                width: ATLAS_PAGE_SIZE,
                height: ATLAS_PAGE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.tile_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atlas page bg"),
            layout: &self.page_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        // pop() 从尾部取 → 倒序填充，让槽位 0 先被使用。
        let free_slots: Vec<u32> = (0..ATLAS_SLOTS_PER_PAGE).rev().collect();
        self.pages.push(AtlasPage {
            texture,
            bind_group,
            free_slots,
        });
        (self.pages.len() - 1) as u32
    }

    fn alloc_slot(&mut self, device: &wgpu::Device) -> (u32, u32) {
        for (i, page) in self.pages.iter_mut().enumerate() {
            if let Some(slot) = page.free_slots.pop() {
                return (i as u32, slot);
            }
        }
        let page = self.create_page(device);
        let slot = self.pages[page as usize]
            .free_slots
            .pop()
            .expect("atlas page 刚创建就为空");
        (page, slot)
    }

    fn free_slot(&mut self, page: u32, slot: u32) {
        if let Some(p) = self.pages.get_mut(page as usize) {
            p.free_slots.push(slot);
        }
    }

    /// 上传/覆盖一块 tile。新 tile 从 atlas 空闲槽位分配。
    fn upload_tile(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, up: &TileUpload) {
        let existing = self.cache.get(&up.key).map(|t| (t.page, t.slot));
        let (page, slot) = match existing {
            Some(p) => p,
            None => self.alloc_slot(device),
        };
        let (sx, sy) = slot_xy(slot);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.pages[page as usize].texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: sx * TILE_SIZE as u32,
                    y: sy * TILE_SIZE as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &up.pixels[..],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((TILE_SIZE * 4) as u32),
                rows_per_image: Some(TILE_SIZE as u32),
            },
            wgpu::Extent3d {
                width: TILE_SIZE as u32,
                height: TILE_SIZE as u32,
                depth_or_array_layers: 1,
            },
        );
        let frame = self.frame;
        match existing {
            Some(_) => {
                let t = self.cache.get_mut(&up.key).expect("刚查到过");
                t.uploaded_rev = up.revision;
                t.last_frame = frame;
            }
            None => {
                self.cache.insert(
                    up.key,
                    GpuTile {
                        page,
                        slot,
                        uploaded_rev: up.revision,
                        last_frame: frame,
                    },
                );
            }
        }
    }

    /// 该 tile 已同步到显存的 CPU revision（`None` = 显存无此 tile）。
    #[inline]
    pub fn uploaded_rev(&self, key: TileKey) -> Option<u64> {
        self.cache.get(&key).map(|t| t.uploaded_rev)
    }

    /// 当前显存驻留的 tile 数。
    #[inline]
    pub fn resident(&self) -> usize {
        self.cache.len()
    }

    /// 已分配的 atlas 页数（诊断用）。
    #[inline]
    pub fn atlas_pages(&self) -> usize {
        self.pages.len()
    }

    /// 清空显存缓存（下一次同步会按需重建全部可见 tile）。
    pub fn invalidate_all(&mut self) {
        self.cache.clear();
        self.pages.clear();
        self.batches.clear();
        self.instance_buf = None;
        self.instance_capacity = 0;
    }

    /// 剔除单个 tile 的显存副本（撤销后画布上已不存在的内容）。
    #[inline]
    pub fn evict(&mut self, key: TileKey) {
        if let Some(t) = self.cache.remove(&key) {
            self.free_slot(t.page, t.slot);
        }
    }

    /// 按「容量 + 保护白名单」做一次 LRU 淘汰。
    fn trim(&mut self, protect: &[TileKey]) {
        let capacity = capacity_for(protect.len());
        if self.cache.len() <= capacity {
            return;
        }
        let entries: Vec<(TileKey, u64)> =
            self.cache.iter().map(|(&k, t)| (k, t.last_frame)).collect();
        for key in plan_lru_evictions(&entries, protect, capacity) {
            self.evict(key);
        }
    }

    fn ensure_instance_buffer(&mut self, device: &wgpu::Device, needed: usize) {
        if self.instance_capacity >= needed {
            return;
        }
        let capacity = needed.next_power_of_two().max(1024);
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tile instances"),
            size: capacity as u64 * INSTANCE_STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buf = Some(buf);
        self.instance_capacity = capacity;
    }

    fn instance_for(&self, draw: DrawTile, tile: &GpuTile) -> Instance {
        let (ox, oy) = draw.key.world_origin();
        let uv = slot_uv(tile.slot);
        let f = |v: u8| v as f32 / TILE_SIZE as f32;
        let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
        Instance {
            origin: [ox as f32, oy as f32],
            scale: (1u32 << draw.key.level) as f32,
            uv_min: [
                mix(uv[0], uv[2], f(draw.sub[0])),
                mix(uv[1], uv[3], f(draw.sub[1])),
            ],
            uv_max: [
                mix(uv[0], uv[2], f(draw.sub[2])),
                mix(uv[1], uv[3], f(draw.sub[3])),
            ],
            _pad: 0.0,
        }
    }

    /// 一帧的准备：同步 tile 显存 + 建立本帧的 instanced 批次。
    ///
    /// `draw` 是本帧实际要绘制的 tile；它们会被按 atlas 页分组，一页一个
    /// draw batch。幂等：同一状态重复调用结果一致。
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uploads: &[TileUpload],
        evict: &[TileKey],
        visible: &[TileKey],
        view_matrix: &[[f32; 4]; 4],
        nearest: bool,
        draw: &[DrawTile],
    ) {
        self.frame += 1;
        for up in uploads {
            self.upload_tile(device, queue, up);
        }
        for &key in evict {
            self.evict(key);
        }
        self.set_view(queue, view_matrix, nearest);
        // 回退取样格（可能来自别的 mip 级别）也是本帧要用的数据，
        // 不能被 LRU 在这中间淘汰。
        let mut protect: Vec<TileKey> = visible.to_vec();
        protect.extend(draw.iter().map(|d| d.source));
        self.trim(&protect);

        self.batches.clear();
        if draw.is_empty() {
            return;
        }
        let mut buckets: Vec<Vec<Instance>> = (0..self.pages.len()).map(|_| Vec::new()).collect();
        for &d in draw {
            let Some(tile) = self.cache.get(&d.source) else {
                continue;
            };
            buckets[tile.page as usize].push(self.instance_for(d, tile));
        }
        let total: usize = buckets.iter().map(Vec::len).sum();
        if total == 0 {
            return;
        }
        self.ensure_instance_buffer(device, total);
        let mut flat: Vec<Instance> = Vec::with_capacity(total);
        for (page, bucket) in buckets.iter().enumerate() {
            if bucket.is_empty() {
                continue;
            }
            let first = flat.len() as u32;
            flat.extend_from_slice(bucket);
            self.batches.push(DrawBatch {
                page: page as u32,
                first,
                count: bucket.len() as u32,
            });
        }
        let mut bytes = Vec::with_capacity(flat.len() * INSTANCE_STRIDE as usize);
        for inst in &flat {
            for v in [
                inst.origin[0],
                inst.origin[1],
                inst.scale,
                inst.uv_min[0],
                inst.uv_min[1],
                inst.uv_max[0],
                inst.uv_max[1],
                inst._pad,
            ] {
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
        let buf = self.instance_buf.as_ref().expect("刚确保过容量");
        queue.write_buffer(buf, 0, &bytes);
    }

    /// 把 prepare 阶段建立的批次画进给定的 render pass。
    ///
    /// 调用方负责创建 pass、设置 viewport/scissor（画布矩形）。`checks`
    /// 为 true 时先铺透明棋盘格。
    pub fn draw_prepared(&self, pass: &mut wgpu::RenderPass<'_>, checks: bool) {
        if checks {
            pass.set_pipeline(&self.check_pipeline);
            pass.draw(0..3, 0..1);
        }
        if self.batches.is_empty() {
            return;
        }
        let Some(buf) = self.instance_buf.as_ref() else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.view_bg, &[]);
        for batch in &self.batches {
            let page = &self.pages[batch.page as usize];
            pass.set_bind_group(1, &page.bind_group, &[]);
            let offset = batch.first as u64 * INSTANCE_STRIDE;
            let end = offset + batch.count as u64 * INSTANCE_STRIDE;
            pass.set_vertex_buffer(0, buf.slice(offset..end));
            pass.draw(0..6, 0..batch.count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hokusai::TiledSurface;
    use pigment_surface::{LayerStack, PigmentSurface};

    fn k(tx: i32, ty: i32) -> TileKey {
        TileKey::new(0, tx, ty)
    }

    fn touch(surface: &mut PigmentSurface, tx: i32, ty: i32) {
        let t = surface.tile_request_start(tx, ty);
        t[0][0] = [100, 200, 300, 1 << 15];
        surface.tile_request_end(tx, ty);
    }

    fn rev(st: &LayerStack, key: TileKey) -> u64 {
        st.tile_composite_revision_at(key.level, key.tx, key.ty)
    }

    fn paint(st: &LayerStack, key: TileKey) -> bool {
        st.has_paint_at_level(key.level, key.tx, key.ty)
    }

    #[test]
    fn tile_format_follows_target_colorspace() {
        assert_eq!(
            tile_format(wgpu::TextureFormat::Bgra8UnormSrgb),
            wgpu::TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(
            tile_format(wgpu::TextureFormat::Bgra8Unorm),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }

    #[test]
    fn key_includes_level() {
        assert_ne!(TileKey::new(0, 0, 0), TileKey::new(1, 0, 0));
        assert_eq!(TileKey::new(0, 1, 0).world_size(), 64);
        assert_eq!(TileKey::new(3, 1, 0).world_size(), 512);
        assert_eq!(TileKey::new(3, 1, 2).world_origin(), (512, 1024));
        assert_eq!(TileKey::new(2, -1, -1).world_origin(), (-256, -256));
    }

    #[test]
    fn atlas_slots_are_unique_and_uv_stays_inside_the_tile() {
        let mut seen = std::collections::HashSet::new();
        for slot in 0..ATLAS_SLOTS_PER_PAGE {
            assert!(seen.insert(slot));
            let (sx, sy) = slot_xy(slot);
            assert!(sx < ATLAS_SLOTS_PER_ROW && sy < ATLAS_SLOTS_PER_ROW);
            let uv = slot_uv(slot);
            assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[2]));
            let expected = [
                (sx * 64) as f32 / ATLAS_PAGE_SIZE as f32,
                (sy * 64) as f32 / ATLAS_PAGE_SIZE as f32,
                (sx * 64 + 64) as f32 / ATLAS_PAGE_SIZE as f32,
                (sy * 64 + 64) as f32 / ATLAS_PAGE_SIZE as f32,
            ];
            assert_eq!(uv, expected);
        }
        assert_eq!(ATLAS_SLOTS_PER_PAGE, 1024);
    }

    #[test]
    fn plans_content_tiles_not_yet_uploaded() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let visible = vec![k(0, 0), k(1, 0), k(2, 0)];
        let plan = plan_uploads(|_| None, &visible, 16, |c| paint(&st, c), |c| rev(&st, c));
        assert_eq!(plan, vec![k(0, 0), k(1, 0)]);
    }

    #[test]
    fn skips_tiles_already_synced() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let visible = vec![k(0, 0), k(1, 0)];
        let uploaded = |c: TileKey| Some(rev(&st, c));
        assert!(
            plan_uploads(uploaded, &visible, 16, |c| paint(&st, c), |c| rev(&st, c)).is_empty()
        );
    }

    #[test]
    fn replans_after_cpu_change() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let mut uploaded: HashMap<TileKey, u64> = HashMap::new();
        for c in [k(0, 0), k(1, 0)] {
            uploaded.insert(c, rev(&st, c));
        }
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let plan = plan_uploads(
            |c| uploaded.get(&c).copied(),
            &[k(0, 0), k(1, 0)],
            16,
            |c| paint(&st, c),
            |c| rev(&st, c),
        );
        assert_eq!(plan, vec![k(1, 0)]);
    }

    #[test]
    fn planning_is_idempotent_across_discarded_passes() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 3, 2);
        let visible = vec![k(0, 0), k(3, 2)];
        let uploaded: HashMap<TileKey, u64> = HashMap::new();
        let probe = |c: TileKey| uploaded.get(&c).copied();
        let pass1 = plan_uploads(probe, &visible, 16, |c| paint(&st, c), |c| rev(&st, c));
        let pass2 = plan_uploads(probe, &visible, 16, |c| paint(&st, c), |c| rev(&st, c));
        assert_eq!(pass1, pass2);
        let mut synced = HashMap::new();
        for c in &pass1 {
            synced.insert(*c, rev(&st, *c));
        }
        assert!(plan_uploads(
            |c| synced.get(&c).copied(),
            &visible,
            16,
            |c| paint(&st, c),
            |c| rev(&st, c)
        )
        .is_empty());
    }

    #[test]
    fn respects_per_frame_cap_without_losing_backlog() {
        let mut st = LayerStack::new();
        let mut visible = Vec::new();
        for tx in 0..10 {
            touch(&mut st.current_mut().surface_mut().unwrap(), tx, 0);
            visible.push(k(tx, 0));
        }
        let uploaded: HashMap<TileKey, u64> = HashMap::new();
        let probe = |c: TileKey| uploaded.get(&c).copied();
        let has = |c: TileKey| st.has_content_at_level(c.level, c.tx, c.ty);
        let first = plan_uploads(probe, &visible, 4, has, |c| rev(&st, c));
        assert_eq!(first, vec![k(0, 0), k(1, 0), k(2, 0), k(3, 0)]);
        let mut synced: HashMap<TileKey, u64> = HashMap::new();
        for c in &first {
            synced.insert(*c, rev(&st, *c));
        }
        let second = plan_uploads(
            |c| synced.get(&c).copied(),
            &visible,
            4,
            has,
            |c| rev(&st, c),
        );
        assert_eq!(second, vec![k(4, 0), k(5, 0), k(6, 0), k(7, 0)]);
    }

    #[test]
    fn plans_eviction_for_tiles_removed_by_undo() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let sshot = st.current().surface().unwrap().save_snapshot();
        touch(&mut st.current_mut().surface_mut().unwrap(), 2, 0);
        let _ = st
            .current_mut()
            .surface_mut()
            .unwrap()
            .load_snapshot(&sshot);
        let visible = vec![k(0, 0), k(1, 0), k(2, 0)];
        let resident = |c: TileKey| matches!(c.tx, 0 | 1 | 2) && c.ty == 0 && c.level == 0;
        let evict = plan_evictions(resident, &visible, |c| paint(&st, c));
        assert_eq!(evict, vec![k(2, 0)]);
    }

    #[test]
    fn does_not_evict_tiles_still_present_on_cpu() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 0);
        let visible = vec![k(0, 0), k(1, 0)];
        assert!(plan_evictions(|_| true, &visible, |c| paint(&st, c)).is_empty());
    }

    #[test]
    fn does_not_evict_when_not_resident() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        let visible = vec![k(0, 0), k(7, 7)];
        assert!(plan_evictions(|_| false, &visible, |c| paint(&st, c)).is_empty());
    }

    #[test]
    fn eviction_is_limited_to_the_visible_set() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        assert!(plan_evictions(|c| c == k(9, 9), &[k(0, 0)], |c| paint(&st, c)).is_empty());
    }

    #[test]
    fn eviction_planning_is_idempotent() {
        let mut st = LayerStack::new();
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);
        let visible = vec![k(0, 0), k(1, 0)];
        let resident = |c: TileKey| c == k(1, 0);
        let a = plan_evictions(resident, &visible, |c| paint(&st, c));
        let b = plan_evictions(resident, &visible, |c| paint(&st, c));
        assert_eq!(a, b);
        assert_eq!(a, vec![k(1, 0)]);
    }

    /// 低缩放下同一 mip 格内的第二次笔迹必须重新上传。
    ///
    /// 回归：旧 `mark_dirty` 遇到已脏的祖先就 break，脏哨兵 revision 又是 0，
    /// GPU 水位与它相等，于是 dab 画进了 CPU 却永远不重传。
    #[test]
    fn second_write_in_same_mip_tile_reuploads() {
        let mut st = LayerStack::new();
        let key = TileKey::new(4, 0, 0);
        touch(&mut st.current_mut().surface_mut().unwrap(), 0, 0);

        let compose_rev = |st: &LayerStack| {
            let _ = st.composite_tile_rgba8_at(4, 0, 0);
            st.tile_composite_revision_at_cached(4, 0, 0)
        };
        let mut uploaded_rev = compose_rev(&st);

        touch(&mut st.current_mut().surface_mut().unwrap(), 1, 1);
        let rev = st.tile_composite_revision_at_cached(4, 0, 0);
        assert_ne!(uploaded_rev, rev, "同一 mip 格内的第二次写入换了 revision");
        let plan = plan_uploads(
            |c| (c == key).then_some(uploaded_rev),
            &[key],
            8,
            |c| st.has_content_at_level_cached(c.level, c.tx, c.ty),
            |c| st.tile_composite_revision_at_cached(c.level, c.tx, c.ty),
        );
        assert_eq!(plan, vec![key], "脏 mip 格必须进入上传计划");

        // 上传后水位更新 → 下一帧不再重复上传。
        uploaded_rev = compose_rev(&st);
        let plan = plan_uploads(
            |c| (c == key).then_some(uploaded_rev),
            &[key],
            8,
            |c| st.has_content_at_level_cached(c.level, c.tx, c.ty),
            |c| st.tile_composite_revision_at_cached(c.level, c.tx, c.ty),
        );
        assert!(plan.is_empty());
    }

    /// 背景格的水位可能是 0；之后画进去必须被识别为变化。
    #[test]
    fn painting_background_only_tile_reuploads_from_zero_watermark() {
        let mut st = LayerStack::new();
        let key = TileKey::new(4, 2, 0);
        assert!(st.has_content_at_level_cached(4, 2, 0), "背景可见");

        let _ = st.composite_tile_rgba8_at(4, 2, 0);
        // 背景格没有图层内容，mip 是 Empty(revision 0)，但合成哈希本身非零；
        // 关键是"画进去之后哈希必须变化"。
        let watermark = st.tile_composite_revision_at_cached(4, 2, 0);

        touch(&mut st.current_mut().surface_mut().unwrap(), 32, 0);
        let rev = st.tile_composite_revision_at_cached(4, 2, 0);
        assert_ne!(rev, watermark);
        let plan = plan_uploads(
            |c| (c == key).then_some(watermark),
            &[key],
            8,
            |c| st.has_content_at_level_cached(c.level, c.tx, c.ty),
            |c| st.tile_composite_revision_at_cached(c.level, c.tx, c.ty),
        );
        assert_eq!(plan, vec![key]);
    }

    #[test]
    fn capacity_follows_the_visible_set() {
        assert_eq!(capacity_for(0), GPU_TILE_CAPACITY);
        assert_eq!(capacity_for(508), GPU_TILE_CAPACITY);
        assert_eq!(capacity_for(1785), 1785 + GPU_TILE_SLACK);
    }

    #[test]
    fn lru_evicts_only_tiles_outside_the_visible_set() {
        let visible = vec![k(0, 0), k(1, 0)];
        let entries = vec![(k(0, 0), 1), (k(1, 0), 2), (k(9, 9), 0), (k(8, 8), 3)];
        assert_eq!(
            plan_lru_evictions(&entries, &visible, 2),
            vec![k(9, 9), k(8, 8)]
        );
        assert!(plan_lru_evictions(&entries, &visible, 4).is_empty());
        assert!(plan_lru_evictions(&entries[..2], &visible, 1).is_empty());
    }

    #[test]
    fn uploads_converge_when_visible_set_exceeds_capacity() {
        let visible: Vec<TileKey> = (0..1785).map(|i| TileKey::new(1, i % 60, i / 60)).collect();
        let mut cache: HashMap<TileKey, u64> = HashMap::new();
        let mut frames = 0;
        loop {
            let plan = plan_uploads(|c| cache.get(&c).copied(), &visible, 128, |_| true, |_| 1);
            if plan.is_empty() {
                break;
            }
            for c in &plan {
                cache.insert(*c, 1);
            }
            let entries: Vec<(TileKey, u64)> = cache.iter().map(|(&c, &f)| (c, f)).collect();
            for c in plan_lru_evictions(&entries, &visible, capacity_for(visible.len())) {
                cache.remove(&c);
            }
            frames += 1;
            assert!(frames <= 32, "必须收敛，不能无限上传/淘汰");
        }
        assert_eq!(frames, 14);
        assert_eq!(cache.len(), 1785);
    }
}
