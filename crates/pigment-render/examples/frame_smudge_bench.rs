//! 帧级延迟 smudge 基准：模拟 240Hz 输入、每帧一次 GPU 同步。
//!
//! 用法：`cargo run --release -p pigment-render --example frame_smudge_bench [events_per_frame] [frames]`
//!
//! 与用户自检同构：事件照常逐个喂给引擎，但**不**在每个 dab 前同步；
//! 一帧结束才 flush 一次，读回 tile 并统计 dabs / 采样 / 队列占用。

use std::time::Instant;

use hokusai::{BrushSetting, BrushState, Dab, TiledSurface};
use pigment_engine::brushlib::BrushLibrary;
use pigment_engine::StrokeEvent;
use pigment_render::{GpuDabEngine, SurfaceGpuSink};
use pigment_surface::PigmentSurface;

struct CountSurface<'a> {
    inner: &'a mut PigmentSurface,
    dabs: u64,
    deferred: u64,
    cpu_samples: u64,
}

impl TiledSurface for CountSurface<'_> {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut hokusai::tile::TilePixels {
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
    fn flush_dabs(&mut self) {
        self.inner.flush_dabs()
    }
    fn tile_lookup(&self, tx: i32, ty: i32) -> Option<&hokusai::tile::TilePixels> {
        self.inner.tile_lookup(tx, ty)
    }
    fn draw_dab(&mut self, dab: &Dab) -> bool {
        self.dabs += 1;
        TiledSurface::draw_dab(self.inner, dab)
    }
    fn get_color_pigment(
        &self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> hokusai::color::RgbaF32 {
        // &self cannot mutate; the count is only reported for the CPU path.
        self.inner.get_color_pigment(x, y, radius, paint)
    }
    fn defer_smudge(&self) -> bool {
        self.inner.defer_smudge()
    }
    fn draw_dab_deferred_smudge(
        &mut self,
        dab: &Dab,
        req: &hokusai::DeferredSmudge,
    ) -> Option<bool> {
        let r = self.inner.draw_dab_deferred_smudge(dab, req);
        if r.is_some() {
            self.deferred += 1;
        }
        r
    }
    fn deferred_smudge_dirty(&self) -> bool {
        self.inner.deferred_smudge_dirty()
    }
    fn sync_deferred_smudge(&mut self, epoch: u64, buckets: &mut [hokusai::SmudgeBucket]) {
        self.inner.sync_deferred_smudge(epoch, buckets)
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let events_per_frame: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(12);
    let frames: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(30);
    let radius: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(100.0);
    let brush_key = args
        .next()
        .unwrap_or_else(|| "classic/blending_knife".to_string());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("frame smudge bench"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .expect("device");

    let lib = BrushLibrary::embedded();
    let entry = lib
        .entries
        .iter()
        .find(|e| e.key == brush_key)
        .unwrap_or_else(|| panic!("brush {brush_key} not found"));
    let mut brush = entry.load().expect("brush");
    let zoom = 0.0625f32;
    brush.get_mut(BrushSetting::Radius).base_value = radius.ln();

    let mut surface = PigmentSurface::new();
    let (w, h) = (4000usize, 1200usize);
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            [40u8.wrapping_add(x), 90u8.wrapping_add(y / 2), 200u8.wrapping_sub(x / 3), 255]
        })
        .collect();
    surface.load_from_rgba8(&px, -25000, -600, w as u32, h as u32);
    let engine = GpuDabEngine::new(&device, &queue).expect("engine");
    surface.set_gpu_sink(Some(Box::new(SurfaceGpuSink::new(engine))));

    let mut state = BrushState::default();
    let mut counts = CountSurface {
        inner: &mut surface,
        dabs: 0,
        deferred: 0,
        cpu_samples: 0,
    };
    let dt = 1.0 / 240.0;
    let screen_step = 25.0f32;
    let world_step = screen_step / zoom;
    let mut x = -13500.0f32;
    let y = 0.0f32;

    let mut apply_us = Vec::new();
    let mut flush_us = Vec::new();
    let mut dabs_per_frame = Vec::new();
    let mut pending = Vec::new();
    for _ in 0..frames {
        let before_dabs = counts.dabs + counts.deferred;
        let t = Instant::now();
        counts.inner.begin_atomic();
        for _ in 0..events_per_frame {
            let ev = StrokeEvent::new(dt, x, y, 0.5, 0.0, 0.0, zoom, 0.0, 0.0);
            let _ = ev.apply(&brush, &mut state, &mut counts);
            x += world_step;
        }
        counts.inner.end_atomic();
        apply_us.push(t.elapsed().as_secs_f64() * 1e6);
        dabs_per_frame.push(counts.dabs + counts.deferred - before_dabs);

        let t = Instant::now();
        counts.inner.flush_gpu_dabs_budgeted();
        flush_us.push(t.elapsed().as_secs_f64() * 1e6);
        pending.push(counts.inner.gpu_pending_ops());
    }
    let _ = state.last_pressure;

    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let max = |v: &[f64]| v.iter().cloned().fold(0.0, f64::max);
    eprintln!(
        "events/frame={events_per_frame} frames={frames} | dabs/frame mean={:.1} max={} | deferred={} cpu_draw={}",
        mean(&dabs_per_frame.iter().map(|&v| v as f64).collect::<Vec<_>>()),
        dabs_per_frame.iter().copied().max().unwrap_or(0),
        counts.deferred,
        counts.dabs,
    );
    eprintln!(
        "apply: mean={:.2}ms max={:.2}ms | flush: mean={:.2}ms max={:.2}ms | total/frame mean={:.2}ms | pending end={} max={}",
        mean(&apply_us) / 1000.0,
        max(&apply_us) / 1000.0,
        mean(&flush_us) / 1000.0,
        max(&flush_us) / 1000.0,
        (mean(&apply_us) + mean(&flush_us)) / 1000.0,
        pending.last().copied().unwrap_or(0),
        pending.iter().copied().max().unwrap_or(0),
    );
}
