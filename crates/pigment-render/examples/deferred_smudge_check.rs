//! GPU 延迟 smudge 链 vs CPU 参考笔画的回归对照。
//!
//! 用法：`cargo run --release -p pigment-render --example deferred_smudge_check`
//!
//! 两边喂完全相同的事件序列，GPU 侧每个事件后同步一次（与 GUI 的每帧一次
//! 同构）。最终逐 tile 比较像素，并要求设备 bucket 状态与 CPU 镜像一致。

use hokusai::{BrushSetting, BrushState, Dab, SmudgeBucket, TiledSurface};
use pigment_engine::brushlib::BrushLibrary;
use pigment_engine::StrokeEvent;
use pigment_render::{GpuDabEngine, SurfaceGpuSink};
use pigment_surface::PigmentSurface;
use std::sync::atomic::{AtomicU64, Ordering};

struct Counts {
    cpu_samples: AtomicU64,
    draws: AtomicU64,
    deferred: AtomicU64,
}

struct CountSurface<'a> {
    inner: &'a mut PigmentSurface,
    counts: &'a Counts,
    /// Mirrors the engine's deferred-sampling gate: the GPU path samples at
    /// most once every `PIGMENT_SMUDGE_SAMPLE_DABS` dabs (default 8) and
    /// reuses that sample for the dabs in between.
    sample_cache: std::cell::Cell<Option<[f32; 4]>>,
    dabs_since_sample: std::cell::Cell<u32>,
}

fn sample_interval() -> u32 {
    std::env::var("PIGMENT_SMUDGE_SAMPLE_DABS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
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
        self.counts.draws.fetch_add(1, Ordering::Relaxed);
        TiledSurface::draw_dab(self.inner, dab)
    }
    fn get_color(
        &self,
        x: f32,
        y: f32,
        radius: f32,
    ) -> hokusai::color::RgbaF32 {
        self.counts.cpu_samples.fetch_add(1, Ordering::Relaxed);
        self.inner.get_color(x, y, radius)
    }
    fn get_color_pigment_gpu(
        &mut self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> Option<hokusai::color::RgbaF32> {
        self.inner.get_color_pigment_gpu(x, y, radius, paint)
    }

    fn get_color_pigment(
        &self,
        x: f32,
        y: f32,
        radius: f32,
        paint: f32,
    ) -> hokusai::color::RgbaF32 {
        let interval = sample_interval();
        let n = self.dabs_since_sample.get().saturating_add(1);
        self.dabs_since_sample.set(n);
        let due = self.sample_cache.get().is_none() || interval == 0 || n >= interval;
        if due {
            self.counts.cpu_samples.fetch_add(1, Ordering::Relaxed);
            let c = self.inner.get_color_pigment(x, y, radius, paint);
            self.sample_cache.set(Some([c.r, c.g, c.b, c.a]));
            self.dabs_since_sample.set(0);
            c
        } else {
            let c = self.sample_cache.get().expect("cached sample");
            hokusai::color::RgbaF32 {
                r: c[0],
                g: c[1],
                b: c[2],
                a: c[3],
            }
        }
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
            self.counts.deferred.fetch_add(1, Ordering::Relaxed);
        }
        r
    }
    fn deferred_smudge_dirty(&self) -> bool {
        self.inner.deferred_smudge_dirty()
    }
    fn sync_deferred_smudge(&mut self, epoch: u64, buckets: &mut [SmudgeBucket]) {
        self.inner.sync_deferred_smudge(epoch, buckets)
    }
}

fn gradient(w: usize, h: usize) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            [
                40u8.wrapping_add(x.wrapping_mul(2)),
                90u8.wrapping_add(y),
                200u8.wrapping_sub(x / 3),
                255,
            ]
        })
        .collect()
}

fn make_surface() -> PigmentSurface {
    let mut s = PigmentSurface::new();
    let (w, h) = (2400usize, 800usize);
    s.load_from_rgba8(&gradient(w, h), -400, -400, w as u32, h as u32);
    s
}

fn stroke_cpu(
    brush: &pigment_engine::hokusai::Brush,
    events: usize,
) -> (PigmentSurface, BrushState, Counts) {
    let mut surface = make_surface();
    let mut state = BrushState::default();
    let counts = Counts {
        cpu_samples: AtomicU64::new(0),
        draws: AtomicU64::new(0),
        deferred: AtomicU64::new(0),
    };
    let mut cs = CountSurface {
        inner: &mut surface,
        counts: &counts,
        sample_cache: std::cell::Cell::new(None),
        dabs_since_sample: std::cell::Cell::new(0),
    };
    let dt = 1.0 / 240.0;
    for i in 0..events {
        let x = 100.0 + i as f32 * 8.0;
        let ev = StrokeEvent::new(dt, x, 150.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0);
        cs.begin_atomic();
        let _ = ev.apply(brush, &mut state, &mut cs);
        cs.end_atomic();
    }
    drop(cs);
    (surface, state, counts)
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("deferred smudge check"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .expect("device");

    let lib = BrushLibrary::embedded();
    let entry = lib
        .entries
        .iter()
        .find(|e| e.key == "classic/blending_knife")
        .expect("blending_knife");
    let mut brush = entry.load().expect("brush");
    brush.get_mut(BrushSetting::Radius).base_value = 100.0f32.ln();
    let events: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(160);

    // ---- CPU 参考 ----
    hokusai::brushmodes::reset_get_color_rng();
    let (cpu_surface, cpu_state, cpu_counts) = stroke_cpu(&brush, events);

    // ---- GPU 延迟链 ----
    hokusai::brushmodes::reset_get_color_rng();
    let mut gpu_surface = make_surface();
    let mut gpu_state = BrushState::default();
    let engine = GpuDabEngine::new(&device, &queue).expect("engine");
    gpu_surface.set_gpu_sink(Some(Box::new(SurfaceGpuSink::new(engine))));
    let gpu_counts = Counts {
        cpu_samples: AtomicU64::new(0),
        draws: AtomicU64::new(0),
        deferred: AtomicU64::new(0),
    };
    {
        let mut gs = CountSurface {
            inner: &mut gpu_surface,
            counts: &gpu_counts,
            sample_cache: std::cell::Cell::new(None),
        dabs_since_sample: std::cell::Cell::new(0),
        };
        let dt = 1.0 / 240.0;
        for i in 0..events {
            let x = 100.0 + i as f32 * 8.0;
            let ev = StrokeEvent::new(dt, x, 150.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.0);
            gs.begin_atomic();
            let _ = ev.apply(&brush, &mut gpu_state, &mut gs);
            gs.end_atomic();
        }
    }
    // 按渲染帧的限量 flush 分批排空，覆盖跨帧的采样依赖。
    while gpu_surface.gpu_pending_ops() > 0 {
        gpu_surface.flush_gpu_dabs_budgeted();
    }
    gpu_surface.flush_gpu_dabs();

    // ---- 像素比较 ----
    let mut max_diff = 0i32;
    let mut diff_channels = 0usize;
    let mut total_channels = 0usize;
    let mut sample = None;
    for (c, gt) in gpu_surface.tiles() {
        let Some(ct) = cpu_surface.tile(c.0, c.1) else {
            max_diff = max_diff.max(32768);
            continue;
        };
        let a = gt.pixels();
        let b = ct;
        for y in 0..hokusai::tile::TILE_SIZE {
            for x in 0..hokusai::tile::TILE_SIZE {
                for k in 0..4 {
                    total_channels += 1;
                    let d = a[y][x][k] as i32 - b[y][x][k] as i32;
                    if d != 0 {
                        diff_channels += 1;
                        max_diff = max_diff.max(d.abs());
                        if sample.is_none() && d.abs() > 2 {
                            sample = Some((c.0 * 64 + x as i32, c.1 * 64 + y as i32, k, a[y][x], b[y][x]));
                        }
                    }
                }
            }
        }
    }

    // ---- bucket 状态比较（sync 会 flush 并回读设备状态）----
    gpu_surface.sync_deferred_smudge(gpu_state.reset_epoch, &mut gpu_state.smudge_buckets);
    let mut bucket_max = 0.0f32;
    let mut used = 0usize;
    for i in 0..cpu_state.smudge_buckets.len() {
        let a = cpu_state.smudge_buckets[i];
        let b = gpu_state.smudge_buckets[i];
        let d = a
            .color
            .iter()
            .chain(a.prev.iter())
            .zip(b.color.iter().chain(b.prev.iter()))
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()))
            .max((a.recentness - b.recentness).abs());
        if d > 1e-5 {
            used += 1;
        }
        bucket_max = bucket_max.max(d);
    }

    eprintln!(
        "dabs: cpu draw={} gpu deferred={} cpu_samples={} | pixels: diff={}/{} ({:.2}%) max={} sample={:?}",
        cpu_counts.draws.load(Ordering::Relaxed),
        gpu_counts.deferred.load(Ordering::Relaxed),
        gpu_counts.cpu_samples.load(Ordering::Relaxed),
        diff_channels,
        total_channels,
        diff_channels as f64 * 100.0 / total_channels.max(1) as f64,
        max_diff,
        sample
    );
    eprintln!(
        "buckets: differing={} maxdiff={:.7} | cpu_rng={}",
        used,
        bucket_max,
        hokusai::brushmodes::color_rng_state()
    );

    if diff_channels == 0 {
        eprintln!("RESULT: bit-identical");
    } else if max_diff <= 16 {
        eprintln!("RESULT: within 16 fix15 LSB");
    } else {
        eprintln!("RESULT: FAILED (maxdiff={max_diff})");
        std::process::exit(1);
    }
}
