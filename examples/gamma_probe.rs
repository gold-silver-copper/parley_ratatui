//! Renders a solid known-color background and reads back the texture bytes to
//! determine whether Vello's output is sRGB-encoded or linear.

use parley_ratatui::ratatui::Terminal;
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, Rgba, TerminalRenderer, TextureReadback,
    TextureTarget, Theme,
};

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("wgpu adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .expect("wgpu device");

    // Mid-gray sRGB background: byte 128 = 0.502 encoded, 0.216 linear.
    let theme = Theme {
        background: Rgba::rgb(128, 128, 128),
        ..Theme::default()
    };
    let backend = ParleyBackend::new(4, 2);
    let mut tui = Terminal::new(backend).expect("terminal");
    let _ = tui.clear();

    let mut renderer = TerminalRenderer::new(FontOptions::default(), theme);
    let (width, height) = renderer.texture_size_for_buffer(tui.backend().buffer());
    let base_color = renderer.theme().background.to_peniko();
    renderer.build_scene_with_elapsed(tui.backend().buffer(), None, false, 0.0);
    let scene = renderer.replace_scene(parley_ratatui::vello::Scene::new());

    let target = TextureTarget::new(
        &device,
        width,
        height,
        wgpu::TextureFormat::Rgba8Unorm,
        "gamma probe",
    );
    let mut gpu = GpuRenderer::new(&device).expect("gpu renderer");
    gpu.render_scene_to_texture_view(
        &device,
        &queue,
        &target.view,
        width,
        height,
        base_color,
        &scene,
    )
    .expect("render");

    let mut readback = TextureReadback::new();
    let mut rgba = Vec::new();
    readback
        .read_texture_to_rgba8_into(&device, &queue, &target, &mut rgba)
        .expect("readback");

    let center = ((height / 2) * width + width / 2) as usize * 4;
    let pixel = &rgba[center..center + 4];
    println!("input sRGB byte: 128 (0.502 encoded, 0.216 linear)");
    println!("texture bytes at center: {pixel:?}");
    let value = pixel[0];
    if value.abs_diff(128) <= 2 {
        println!("=> Vello output is sRGB-ENCODED (pass-through, display-ready)");
    } else if value.abs_diff(55) <= 3 {
        println!("=> Vello output is LINEAR");
    } else {
        println!("=> unexpected value, inspect manually");
    }
}
