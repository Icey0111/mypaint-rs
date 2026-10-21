//! 单 dab GPU/CPU 逐像素对比（调试用）。

use hokusai::brushmodes::{draw_dab_default, prepare_dab};
use hokusai::color::RgbaF32;
use hokusai::{Dab, TiledSurface};
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
        label: Some("check"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        ..Default::default()
    }))
    .unwrap();

    let color_a: f32 = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);

    let mut gpu = PigmentSurface::new();
    let px: Vec<u8> = (0..64 * 64)
        .flat_map(|i| {
            let x = i % 64;
            let y = i / 64;
            [40 + (x * 3) as u8, 90 + (y * 2) as u8, 160, 255]
        })
        .collect();
    gpu.load_from_rgba8(&px, 0, 0, 64, 64);
    let mut cpu = gpu.clone();

    let dab = Dab {
        x: 32.0,
        y: 32.0,
        radius: 20.0,
        color: RgbaF32 {
            r: 0.85,
            g: 0.35,
            b: 0.1,
            a: 1.0,
        },
        opaque: 0.9,
        hardness: 0.8,
        softness: 0.0,
        alpha_eraser: color_a,
        aspect_ratio: 1.3,
        angle: 25.0,
        lock_alpha: 0.0,
        colorize: 0.0,
        posterize: 0.0,
        posterize_num: 1.0,
        paint: 1.0,
        anti_aliasing: 1.0,
    };
    let prep = prepare_dab(&dab).expect("preparable");
    eprintln!("gpu params: {:?}", prep.gpu_params().map(|p| p.color_a));

    let engine = GpuDabEngine::new(&device, &queue).expect("engine");
    gpu.set_gpu_sink(Some(Box::new(SurfaceGpuSink::new(engine))));
    gpu.begin_atomic();
    gpu.draw_dab(&dab);
    gpu.end_atomic();
    // `end_atomic` 不再同步（一帧一次），读像素前必须显式落地。
    gpu.flush_gpu_dabs();

    let mut any = draw_dab_default(&mut cpu, &dab);
    any |= false;

    let a = gpu.tile(0, 0).expect("gpu tile");
    let b = cpu.tile(0, 0).expect("cpu tile");
    let mut maxd = 0i32;
    let mut ndiff = 0;
    let mut first = None;
    for y in 0..64 {
        for x in 0..64 {
            for k in 0..4 {
                let d = a[y][x][k] as i32 - b[y][x][k] as i32;
                if d != 0 {
                    ndiff += 1;
                    maxd = maxd.max(d.abs());
                    if first.is_none() {
                        first = Some((x, y, k, a[y][x], b[y][x]));
                    }
                }
            }
        }
    }
    eprintln!(
        "cpu painted={any} diff_channels={ndiff}/16384 maxdiff={maxd} first={first:?}"
    );
    for (x, y) in [(32, 32), (20, 32), (32, 20), (45, 32), (32, 45)] {
        eprintln!("  ({x},{y}) gpu={:?} cpu={:?}", a[y][x], b[y][x]);
    }
}
