//! CPU 并行采样 vs GPU 串行采样的耗时对照。
//!
//! 用法：`cargo run --release -p pigment-render --example sample_bench [半径...]`
//!
//! 这个基准回答一个具体问题：把 smudge 采样搬上 GPU 之后，**单次采样**
//! 会不会比 CPU 的并行版本更慢。GPU 侧目前是单工作组单线程（为了保证与
//! `get_color_pigment_default` 的抽样序列、累加顺序完全一致），扫描面积随
//! 半径平方增长，所以必须实测而不是拍脑袋。

use hokusai::brushmodes::reset_get_color_rng;
use hokusai::{Dab, TiledSurface};
use hokusai::color::RgbaF32;
use pigment_render::{GpuDabEngine, SurfaceGpuSink};
use pigment_surface::PigmentSurface;

fn main() {
    let radii: Vec<f32> = std::env::args()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    let radii = if radii.is_empty() {
        vec![20.0, 50.0, 100.0, 200.0, 400.0]
    } else {
        radii
    };

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sample bench"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .unwrap();

    // 2000×2000 画布，够 r=400 的采样圆完全落在里面。
    let mut surface = PigmentSurface::new();
    let (w, h) = (2000usize, 2000usize);
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let x = (i % w) as u8;
            let y = (i / w) as u8;
            [
                40u8.wrapping_add(x),
                90u8.wrapping_add(y / 2),
                200u8.wrapping_sub(y / 3),
                255,
            ]
        })
        .collect();
    surface.load_from_rgba8(&px, 0, 0, w as u32, h as u32);

    let engine = GpuDabEngine::new(&device, &queue).expect("engine");
    surface.set_gpu_sink(Some(Box::new(SurfaceGpuSink::new(engine))));

    // 铺一层内容，让采样有东西可读。
    for i in 0..200 {
        let dab = Dab {
            x: 200.0 + (i as f32 * 8.0),
            y: 900.0 + (i as f32 * 0.7).sin() * 200.0,
            radius: 40.0,
            color: RgbaF32 {
                r: 0.85,
                g: 0.35,
                b: 0.2,
                a: 1.0,
            },
            opaque: 0.6,
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

    eprintln!("radius |   cpu |   gpu | cpu/gpu");
    for &r in &radii {
        let (cx, cy) = (1000.0f32, 1000.0f32);
        // 预热一次（GPU 侧第一次要建 binding 之外的管线状态）。
        reset_get_color_rng();
        let _ = surface.sample_gpu(cx, cy, r, 1.0);

        let n = 8;
        let t = std::time::Instant::now();
        for _ in 0..n {
            reset_get_color_rng();
            let _ = surface.get_color_pigment(cx, cy, r, 1.0);
        }
        let cpu = t.elapsed().as_secs_f64() * 1e3 / n as f64;

        let t = std::time::Instant::now();
        for _ in 0..n {
            let _ = surface.sample_gpu(cx, cy, r, 1.0);
        }
        let gpu = t.elapsed().as_secs_f64() * 1e3 / n as f64;

        eprintln!("{r:6.0} | {cpu:5.2} | {gpu:5.2} | {:5.2}", cpu / gpu);
    }
}
