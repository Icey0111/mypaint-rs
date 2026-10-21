//! GPU smudge 采样内核 vs CPU `get_color_pigment` 的逐位对照。
//!
//! 用法：`cargo run --release -p pigment-render --example gpu_sample_check [N]`
//!
//! 两边从**同一个 RNG 状态**出发（`reset_get_color_rng`），所以抽样点集合
//! 应当完全一致；剩下的差异只可能来自浮点运算顺序。

use hokusai::brushmodes::reset_get_color_rng;
use hokusai::{Dab, TiledSurface};
use hokusai::color::RgbaF32;
use pigment_render::{GpuDabEngine, SurfaceGpuSink};
use pigment_surface::PigmentSurface;

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))
    .unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sample check"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .unwrap();

    // 一块有内容的画布：渐变底 + 几个 dab，让采样结果有真实的空间变化。
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
            color: RgbaF32 {
                r: 0.9,
                g: 0.2 + i as f32 * 0.02,
                b: 0.15,
                a: 1.0,
            },
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

    // 简单伪随机采样点。
    let mut seed = 12345u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / 16777216.0
    };

    let mut worst = 0.0f32;
    let mut worst_at = None;
    let mut sum_err = 0.0f64;
    let mut checked = 0usize;
    let mut gpu_none = 0usize;

    for _ in 0..n {
        let r = 3.0 + next() * 57.0;
        let x = 40.0 + next() * 180.0;
        let y = 40.0 + next() * 180.0;

        reset_get_color_rng();
        let cpu: RgbaF32 = surface.get_color_pigment(x, y, r, 1.0);
        reset_get_color_rng();
        let Some(gpu) = surface.sample_gpu(x, y, r, 1.0) else {
            gpu_none += 1;
            continue;
        };

        let d = [
            (cpu.r - gpu[0]).abs(),
            (cpu.g - gpu[1]).abs(),
            (cpu.b - gpu[2]).abs(),
            (cpu.a - gpu[3]).abs(),
        ];
        let m = d.iter().cloned().fold(0.0f32, f32::max);
        sum_err += m as f64;
        checked += 1;
        if m > worst {
            worst = m;
            worst_at = Some((x, y, r, cpu, gpu));
        }
        if m > 0.02 {
            eprintln!(
                "  ({x:.1},{y:.1}) r={r:.1} cpu=({:.4},{:.4},{:.4},{:.4}) gpu=({:.4},{:.4},{:.4},{:.4}) d={:.4}",
                cpu.r, cpu.g, cpu.b, cpu.a, gpu[0], gpu[1], gpu[2], gpu[3], m
            );
        }
    }

    eprintln!(
        "checked={checked} gpu_unsupported={gpu_none} maxdiff={worst:.5} mean={:.5}",
        if checked > 0 { sum_err / checked as f64 } else { 0.0 }
    );
    if let Some((x, y, r, cpu, gpu)) = worst_at {
        eprintln!(
            "worst at ({x:.1},{y:.1}) r={r:.1}: cpu=({:.4},{:.4},{:.4},{:.4}) gpu=({:.4},{:.4},{:.4},{:.4})",
            cpu.r, cpu.g, cpu.b, cpu.a, gpu[0], gpu[1], gpu[2], gpu[3]
        );
    }
}
