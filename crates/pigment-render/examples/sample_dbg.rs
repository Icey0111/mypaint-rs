//! 临时诊断：对比 GPU 采样内核的 per-tile 覆盖计数与 CPU 实现。
//!
//! 用法：`PIGMENT_SAMPLE_DBG=1 cargo run --release -p pigment-render --example sample_dbg`

use hokusai::brushmodes::{for_each_mask_pixel, reset_get_color_rng};
use hokusai::{Dab, TiledSurface};
use hokusai::color::RgbaF32;
use pigment_render::{GpuDabEngine, SurfaceGpuSink};
use pigment_surface::PigmentSurface;

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sample dbg"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .unwrap();

    let mut surface = PigmentSurface::new();
    let (w, h) = (256usize, 256usize);
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            [40u8.wrapping_add(x), 90u8.wrapping_add(y / 2), 200u8.wrapping_sub(y / 3), 255]
        })
        .collect();
    surface.load_from_rgba8(&px, 0, 0, w as u32, h as u32);

    let engine = GpuDabEngine::new(&device, &queue).expect("engine");
    surface.set_gpu_sink(Some(Box::new(SurfaceGpuSink::new(engine))));

    for i in 0..24 {
        let dab = Dab {
            x: 40.0 + (i as f32 * 8.0),
            y: 120.0 + (i as f32 * 1.5),
            radius: 22.0,
            color: RgbaF32 { r: 0.9, g: 0.2, b: 0.15, a: 1.0 },
            opaque: 0.5,
            hardness: 0.6,
            softness: 0.0,
            alpha_eraser: 0.0,
            aspect_ratio: 1.0,
            angle: 0.0,
            lock_alpha: 0.0,
            colorize: 0.0,
            posterize: 0.0,
            posterize_num: 1.0,
            paint: 1.0,
            anti_aliasing: 1.0,
        };
        surface.begin_atomic();
        surface.draw_dab(&dab);
        surface.end_atomic();
    }
    surface.flush_gpu_dabs();

    let (x, y, r) = (140.7f32, 78.4f32, 51.0f32);
    let ts = 64i32;
    let rf = r + 1.0;
    let tx0 = ((x - rf).floor() as i32).div_euclid(ts);
    let tx1 = ((x + rf).floor() as i32).div_euclid(ts);
    let ty0 = ((y - rf).floor() as i32).div_euclid(ts);
    let ty1 = ((y + rf).floor() as i32).div_euclid(ts);
    eprintln!("bbox tiles x {tx0}..={tx1} y {ty0}..={ty1}");
    let mut cpu_counts = Vec::new();
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let mut n = 0usize;
            for_each_mask_pixel(x, y, r, 0.5, tx, ty, |_, _, _| n += 1);
            cpu_counts.push(n);
        }
    }
    eprintln!("cpu counts={cpu_counts:?}");

    // CPU 侧的采样点序列（复刻 get_color_pigment_parallel 的收集顺序）。
    reset_get_color_rng();
    {
        use hokusai::brushmodes::{
            color_rand_step, color_rng_advance, color_rng_state, set_color_rng_state,
        };
        let interval: u32 = (r * 7.0) as u32;
        let threshold = ((1.0 / (7.0 * r)) * 32767.0) as i32;
        let mut state = color_rng_state();
        let mut pts: Vec<[u32; 4]> = Vec::new();
        for (i, _) in cpu_counts.iter().enumerate() {
            let start = state;
            let interval_samples = if cpu_counts[i] == 0 {
                0
            } else {
                1 + (cpu_counts[i] - 1) / interval as usize
            };
            state = color_rng_advance(state, (cpu_counts[i] - interval_samples) as u64);
            let (utx, uty) = (tx0 + (i as i32 % (tx1 - tx0 + 1)), ty0 + (i as i32 / (tx1 - tx0 + 1)));
            let tile = surface.tile(utx, uty);
            let mut rng = start;
            let mut counter = 0u32;
            for_each_mask_pixel(x, y, r, 0.5, utx, uty, |lx, ly, mask| {
                let sampled = counter == 0
                    || (interval > 1 && color_rand_step(&mut rng) < threshold);
                if sampled {
                    let p = tile.map(|t| t[ly][lx]).unwrap_or([0, 0, 0, 0]);
                    pts.push([
                        mask,
                        p[0] as u32 | ((p[1] as u32) << 16),
                        p[2] as u32 | ((p[3] as u32) << 16),
                        0,
                    ]);
                }
                counter += 1;
                if counter == interval {
                    counter = 0;
                }
            });
        }
        set_color_rng_state(state);
        eprintln!("cpu pts[0..4]={:?}", &pts[..pts.len().min(4)]);
        eprintln!("cpu total={}", pts.len());
    }

    reset_get_color_rng();
    let cpu = surface.get_color_pigment(x, y, r, 1.0);
    reset_get_color_rng();
    let gpu = surface.sample_gpu(x, y, r, 1.0).unwrap();
    eprintln!("cpu={cpu:?}");
    eprintln!("gpu={gpu:?}");
}
