//! 临时基准：真实引擎 + 真实 wgpu 上层，把一笔的成本拆开。
//!
//! `cargo run --release -p pigment-render --example bench_canvas -- <brush-key> <radius> <zoom> <events> <screen-px-per-event>`

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pigment_engine::brushlib::BrushLibrary;
use pigment_engine::hokusai::{BrushSetting, BrushState, Dab, TiledSurface};
use pigment_engine::hokusai::tile::TilePixels;
use pigment_engine::{Brushwork, Document, StrokeEvent};
use pigment_render::{prepare_canvas, DisplayFilter, TileRenderer, View};
use pigment_surface::PigmentSurface;

#[derive(Default)]
struct Timing {
    sample_ns: u128,
    draw_ns: u128,
    sample_calls: u64,
    draw_calls: u64,
}

struct TimingSurface<'a> {
    inner: &'a mut PigmentSurface,
    t: Timing,
}

impl TiledSurface for TimingSurface<'_> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels {
        self.inner.tile_request_start(tx, ty)
    }
    fn tile_request_end(&mut self, tx: i32, ty: i32) {
        self.inner.tile_request_end(tx, ty)
    }
    fn begin_atomic(&mut self) {
        self.inner.begin_atomic()
    }
    fn end_atomic(&mut self) -> Vec<(i32, i32)> {
        self.inner.end_atomic()
    }
    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&TilePixels> {
        self.inner.tile_lookup(tx, ty)
    }
    fn draw_dab(&mut self, dab: &Dab) -> bool {
        let t = Instant::now();
        let r = self.inner.draw_dab(dab);
        DRAW_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        DRAW_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
    fn get_color(&self, x: f32, y: f32, radius: f32) -> pigment_engine::hokusai::color::RgbaF32 {
        let t = Instant::now();
        let r = self.inner.get_color(x, y, radius);
        LEGACY_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        LEGACY_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
    fn get_color_pigment(
        &self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> pigment_engine::hokusai::color::RgbaF32 {
        let t = Instant::now();
        let r = self.inner.get_color_pigment(x, y, radius, paint);
        SAMPLE_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        SAMPLE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        r
    }
}

static SAMPLE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SAMPLE_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LEGACY_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LEGACY_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DRAW_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DRAW_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn main() {
    let mut args = std::env::args().skip(1);
    let brush_key = args.next().unwrap_or_else(|| "classic/blending_knife".into());
    let radius: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(400.0);
    let zoom: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0.0625);
    let events: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(60);
    let screen_step: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(25.0);

    let lib = BrushLibrary::embedded();
    let entry = lib
        .entries
        .iter()
        .find(|e| e.key == brush_key)
        .unwrap_or_else(|| panic!("brush {brush_key} not found"));
    let mut brush = entry.load().unwrap();
    // `BrushSetting::Radius` is radius_logarithmic: base_radius = exp(value).
    brush.get_mut(BrushSetting::Radius).base_value = radius.ln();
    eprintln!(
        "smudge_length setting: {:?}",
        brush.get(BrushSetting::SmudgeLength)
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("adapter");
    let info = adapter.get_info();
    eprintln!(
        "adapter: {:?} {} ({})",
        info.backend, info.name, info.driver
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bench"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .expect("device");

    let width = 1920u32;
    let height = 1080u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let renderer = Arc::new(Mutex::new(TileRenderer::new(
        &device,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    )));

    let gpu_engine = if std::env::var_os("BENCH_GPU").is_some() {
        let e = pigment_render::GpuDabEngine::new(&device, &queue);
        eprintln!("gpu dab engine: {}", if e.is_some() { "on" } else { "unavailable" });
        e
    } else {
        None
    };

    let mut doc = Document::new();
    if std::env::var_os("BENCH_BG").is_some() {
        // 不透明底图：让每个像素的 a0>0，走真正需要光谱混合的分支。
        let (w, h) = (8000u32, 2000u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[210, 130, 60, 255]);
        }
        doc.surface_mut().load_from_rgba8(&px, -14000, -1000, w, h);
    }
    if let Some(engine) = &gpu_engine {
        doc.surface_mut().set_gpu_sink(Some(Box::new(
            pigment_render::SurfaceGpuSink::new(engine.clone()),
        )));
    }
    let mut view = View::default();
    view.zoom = zoom;
    view.offset = [width as f32 * 0.5, height as f32 * 0.5];

    let start_world = view.screen_to_world(100.0, height as f32 * 0.5);
    let ppp = view.ppp;
    let mut state = BrushState::default();
    let mut bw = Brushwork::new();
    let before_sshot = doc.surface_mut().save_snapshot();
    let _ = &mut bw;
    let mut x = start_world[0];
    let y = start_world[1];
    let doc_step = screen_step / zoom * ppp;
    let dtime = 1.0 / 60.0;

    let mut engine_us = Vec::new();
    let mut flush_us = Vec::new();
    let mut prep_us = Vec::new();
    let mut gpu_us = Vec::new();
    let mut upload_counts = Vec::new();

    for i in 0..=events {
        // ---- 事件：真实 Brushwork 路径（含原子窗口、快照 COW）----
        let ev = StrokeEvent::new(dtime, x, y, 1.0, 0.0, 0.0, zoom, 0.0, 0.0);
        let t = Instant::now();
        {
            let surface = doc.surface_mut();
            let mut ts = TimingSurface {
                inner: surface,
                t: Timing::default(),
            };
            ts.begin_atomic();
            let painted = ev.apply(&brush, &mut state, &mut ts);
            ts.end_atomic();
            if i < 2 {
                let sv = &state.prev_settings;
                eprintln!(
                    "apply[{i}] painted={painted} smudge={} len={} paint={} radius_log={}",
                    sv.get(BrushSetting::Smudge),
                    sv.get(BrushSetting::SmudgeLength),
                    sv.get(BrushSetting::Paint),
                    sv.get(BrushSetting::Radius),
                );
            }
        }
        engine_us.push(t.elapsed().as_secs_f64() * 1e6);
        x += doc_step;

        // ---- 帧边界：一次 submit+wait 落地本帧全部排队 dab（GUI 同款）----
        // 这一步必须紧贴在渲染之前：`end_atomic` 故意不同步，队列在
        // `prepare_canvas` 之前落地，采样/收笔前的 flush 在引擎里。
        let t = Instant::now();
        doc.surface_mut().flush_gpu_dabs();
        flush_us.push(t.elapsed().as_secs_f64() * 1e6);

        // ---- 一帧：CPU 合成/编码 → 上传 → GPU 绘制 ----
        let t = Instant::now();
        let mut frame = prepare_canvas(
            &renderer,
            doc.layers(),
            &view,
            [0.0, 0.0, width as f32, height as f32],
            false,
            false,
            None,
            DisplayFilter::None,
        );
        upload_counts.push(frame.uploads.len());
        let t_prep = t.elapsed();
        let pixels: u64 = frame
            .uploads
            .iter()
            .map(|_| (hokusai::tile::TILE_SIZE * hokusai::tile::TILE_SIZE) as u64)
            .sum();

        let t = Instant::now();
        {
            let mut r = renderer.lock().unwrap();
            r.prepare(
                &device,
                &queue,
                &frame.uploads,
                &frame.evict,
                &frame.visible,
                &frame.view_matrix,
                frame.nearest_filter,
                &frame.draw,
            );
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            renderer.lock().unwrap().draw_prepared(&mut pass, false);
        }
        queue.submit(Some(encoder.finish()));
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();
        let gpu = t.elapsed();
        upload_counts.last_mut().map(|_| ());
        prep_us.push(t_prep.as_secs_f64() * 1e6);
        gpu_us.push(gpu.as_secs_f64() * 1e6);
        let _ = pixels;
        frame.uploads.clear();
    }

    // ---- 抬笔：stroke 快照 + StrokeShape diff（stop_recording 的主体） ----
    let t = Instant::now();
    doc.surface_mut().flush_gpu_dabs(); // stop_recording 的第一步
    let t_flush_release = t.elapsed();
    let t = Instant::now();
    let after_sshot = doc.surface_mut().save_snapshot();
    let t_snapshot = t.elapsed();
    let t = Instant::now();
    let shape = pigment_surface::StrokeShape::from_snapshots(&before_sshot, &after_sshot);
    let t_shape = t.elapsed();
    let release_us = t_flush_release.as_secs_f64() * 1e6;
    eprintln!(
        "release split: flush={:.2}ms snapshot={:.2}ms shape_diff={:.2}ms (tiles={})",
        release_us / 1000.0,
        t_snapshot.as_secs_f64() * 1e3,
        t_shape.as_secs_f64() * 1e3,
        shape.as_ref().map(|s| s.tiles.len()).unwrap_or(0)
    );
    let ld = |a: &std::sync::atomic::AtomicU64| a.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!(
        "shape tiles={} | pigment sample calls={} {:.1}ms | legacy calls={} {:.1}ms | draw calls={} {:.1}ms",
        shape.as_ref().map(|s| s.tiles.len()).unwrap_or(0),
        ld(&SAMPLE_CALLS),
        ld(&SAMPLE_NS) as f64 / 1e6,
        ld(&LEGACY_CALLS),
        ld(&LEGACY_NS) as f64 / 1e6,
        ld(&DRAW_CALLS),
        ld(&DRAW_NS) as f64 / 1e6,
    );

    // 再跑几帧让上传收敛，统计收尾后的帧成本。
    // `BENCH_TAIL_ZOOM` 在收笔后切到另一个缩放，复现“缩放换 mip 级别”的补齐过程。
    if let Some(z) = std::env::var("BENCH_TAIL_ZOOM")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        view.zoom = z;
        eprintln!("tail zoom -> {z}");
    }
    let mut tail_us = Vec::new();
    for i in 0..60 {
        let t = Instant::now();
        let mut frame = prepare_canvas(
            &renderer,
            doc.layers(),
            &view,
            [0.0, 0.0, width as f32, height as f32],
            false,
            i == 0,
            None,
            DisplayFilter::None,
        );
        if std::env::var_os("BENCH_TAIL_LOG").is_some() || i == 0 {
            eprintln!(
                "tail[{i}] uploads={} visible={} draw={} uncovered={} pending={}",
                frame.uploads.len(),
                frame.visible.len(),
                frame.draw.len(),
                frame.uncovered,
                frame.pending_uploads
            );
        }
        {
            let mut r = renderer.lock().unwrap();
            r.prepare(
                &device,
                &queue,
                &frame.uploads,
                &frame.evict,
                &frame.visible,
                &frame.view_matrix,
                frame.nearest_filter,
                &frame.draw,
            );
        }
        tail_us.push(t.elapsed().as_secs_f64() * 1e6);
        if frame.pending_uploads == 0 {
            eprintln!("tail settled at frame {i}");
            break;
        }
        frame.uploads.clear();
    }

    if gpu_engine.is_some() {
        // CPU 参考重放：逐字节比较 GPU 结果，确认语义没有被设备端改写。
        let mut ref_doc = Document::new();
        if std::env::var_os("BENCH_BG").is_some() {
            let (w, h) = (8000u32, 2000u32);
            let mut px = vec![0u8; (w * h * 4) as usize];
            for p in px.chunks_exact_mut(4) {
                p.copy_from_slice(&[210, 130, 60, 255]);
            }
            ref_doc.surface_mut().load_from_rgba8(&px, -14000, -1000, w, h);
        }
        let mut rs = BrushState::default();
        let mut rx = start_world[0];
        for _ in 0..=events {
            let ev = StrokeEvent::new(dtime, rx, y, 1.0, 0.0, 0.0, zoom, 0.0, 0.0);
            ev.apply(&brush, &mut rs, ref_doc.surface_mut());
            rx += doc_step;
        }
        let mut bad_tiles = 0usize;
        let mut bad_bytes = 0usize;
        let mut max_diff = 0i32;
        let mut sample: Option<(i32, i32, u16, u16)> = None;
        for (c, t) in doc.surface().tiles() {
            let a = pigment_surface::tile_bytes(t.pixels());
            let b = ref_doc
                .surface()
                .tiles()
                .find(|(rc, _)| *rc == c)
                .map(|(_, rt)| pigment_surface::tile_bytes(rt.pixels()));
            match b {
                Some(b) if a == b => {}
                Some(b) => {
                    bad_tiles += 1;
                    let av = t.pixels();
                    let rv = ref_doc
                        .surface()
                        .tiles()
                        .find(|(rc, _)| *rc == c)
                        .map(|(_, rt)| rt.pixels().clone())
                        .unwrap();
                    for y in 0..64 {
                        for x in 0..64 {
                            for k in 0..4 {
                                let d = av[y][x][k] as i32 - rv[y][x][k] as i32;
                                if d != 0 {
                                    bad_bytes += 1;
                                    max_diff = max_diff.max(d.abs());
                                    if sample.is_none() {
                                        sample = Some((c.0 * 64 + x as i32, c.1 * 64 + y as i32, av[y][x][k], rv[y][x][k]));
                                    }
                                }
                            }
                        }
                    }
                }
                None => bad_tiles += 1,
            }
        }
        eprintln!("verify: differing tiles={bad_tiles} channels={bad_bytes} maxdiff={max_diff} sample={sample:?}");
    }

    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
    eprintln!(
        "brush={brush_key} radius={radius} zoom={zoom} events={} step={screen_step}px dabs/total",
        events + 1
    );
    eprintln!(
        "engine: mean={:.2}ms max={:.2}ms | flush(gpu sync): mean={:.2}ms max={:.2}ms | prepare: mean={:.2}ms max={:.2}ms | gpu: mean={:.2}ms max={:.2}ms",
        mean(&engine_us) / 1000.0,
        max(&engine_us) / 1000.0,
        mean(&flush_us) / 1000.0,
        max(&flush_us) / 1000.0,
        mean(&prep_us) / 1000.0,
        max(&prep_us) / 1000.0,
        mean(&gpu_us) / 1000.0,
        max(&gpu_us) / 1000.0
    );
    eprintln!(
        "uploads: mean={:.1} max={} | release(stop_recording+commit)={:.2}ms | tail frames={} mean={:.2}ms max={:.2}ms | mip rebuilds={}",
        mean(&upload_counts.iter().map(|&v| v as f64).collect::<Vec<_>>()),
        upload_counts.iter().copied().max().unwrap_or(0),
        release_us / 1000.0,
        tail_us.len(),
        mean(&tail_us) / 1000.0,
        max(&tail_us) / 1000.0,
        doc.surface().mip_rebuild_count()
    );
    let _ = Duration::from_secs(0);
}
