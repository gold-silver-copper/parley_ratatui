use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph, Widget};
use parley_ratatui::vello::peniko::Color as VelloColor;
use parley_ratatui::vello::wgpu;
use parley_ratatui::vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureReadback, TextureTarget,
    Theme,
};

const OUTPUT_DIR: &str = "/tmp/parley_ratatui_probe";
const COLUMNS: u16 = 84;
const ROWS: u16 = 12;

fn main() {
    let mut gpu = pollster::block_on(ProbeGpu::new());
    let mut rows = Vec::new();

    for font_size in 8..=48 {
        let msaa = render_case(&mut gpu, font_size as f32, AaConfig::Msaa8);
        let area = render_case(&mut gpu, font_size as f32, AaConfig::Area);
        let default = render_default_case(&mut gpu, font_size as f32);
        rows.push(Comparison::new(font_size, msaa, area, default));
    }

    rows.sort_by(|a, b| b.msaa_penalty.total_cmp(&a.msaa_penalty));
    print_report(&rows);
    write_snapshots(&rows, 8).expect("write probe snapshots");
}

struct ProbeGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    default_renderer: GpuRenderer,
    target: TextureTarget,
    readback: TextureReadback,
    rgba: Vec<u8>,
}

impl ProbeGpu {
    async fn new() -> Self {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("wgpu adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("wgpu device");
        let renderer = Renderer::new(
            &device,
            RendererOptions {
                antialiasing_support: AaSupport::all(),
                ..RendererOptions::default()
            },
        )
        .expect("vello renderer");
        let default_renderer = GpuRenderer::new(&device).expect("default renderer");
        let target = TextureTarget::new(
            &device,
            1,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
            Some("parley_ratatui.render_probe"),
        );

        Self {
            device,
            queue,
            renderer,
            default_renderer,
            target,
            readback: TextureReadback::new(),
            rgba: Vec::new(),
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if self.target.width == width && self.target.height == height {
            return;
        }

        self.target = TextureTarget::new(
            &self.device,
            width,
            height,
            self.target.format,
            Some("parley_ratatui.render_probe"),
        );
        self.readback = TextureReadback::new();
        self.rgba.clear();
    }

    fn render_scene(&mut self, scene: &Scene, aa: AaConfig) -> Vec<u8> {
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &self.target.view,
                &RenderParams {
                    base_color: VelloColor::BLACK,
                    width: self.target.width,
                    height: self.target.height,
                    antialiasing_method: aa,
                },
            )
            .expect("render scene");
        self.readback
            .read_texture_to_rgba8_into(&self.device, &self.queue, &self.target, &mut self.rgba)
            .expect("read rendered texture");
        self.rgba.clone()
    }
}

#[derive(Clone)]
struct RenderCase {
    font_size: u32,
    width: u32,
    height: u32,
    cell_width: f32,
    cell_height: f32,
    stats: ImageStats,
    rgba: Vec<u8>,
}

#[derive(Clone)]
struct Comparison {
    font_size: u32,
    msaa: RenderCase,
    area: RenderCase,
    default: RenderCase,
    msaa_penalty: f32,
    default_area_delta: f32,
}

impl Comparison {
    fn new(font_size: u32, msaa: RenderCase, area: RenderCase, default: RenderCase) -> Self {
        let msaa_penalty = msaa.stats.hard_edge_ratio - area.stats.hard_edge_ratio
            + area
                .stats
                .gray_levels
                .saturating_sub(msaa.stats.gray_levels) as f32
                / 255.0;
        let default_area_delta = (default.stats.hard_edge_ratio - area.stats.hard_edge_ratio).abs()
            + (default.stats.gray_levels as f32 - area.stats.gray_levels as f32).abs() / 255.0;
        Self {
            font_size,
            msaa,
            area,
            default,
            msaa_penalty,
            default_area_delta,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ImageStats {
    coverage: f32,
    edge_density: f32,
    hard_edge_ratio: f32,
    midtone_ratio: f32,
    gray_levels: usize,
}

fn render_case(gpu: &mut ProbeGpu, font_size: f32, aa: AaConfig) -> RenderCase {
    let mut terminal = Terminal::new(ParleyBackend::new(COLUMNS, ROWS)).expect("terminal");
    let mut terminal_renderer =
        TerminalRenderer::new(example_font_options(font_size), Theme::default());
    terminal
        .draw(|frame| ProbePattern { font_size }.render(frame.area(), frame.buffer_mut()))
        .expect("draw probe pattern");

    let buffer = terminal.backend().buffer();
    let (width, height) = terminal_renderer.texture_size_for_buffer(buffer);
    gpu.resize(width, height);
    let scene = terminal_renderer.build_scene(buffer, None, false);
    let rgba = gpu.render_scene(scene, aa);
    let stats = analyze_image(&rgba, width, height);
    let metrics = terminal_renderer.metrics();

    RenderCase {
        font_size: font_size as u32,
        width,
        height,
        cell_width: metrics.cell_width,
        cell_height: metrics.cell_height,
        stats,
        rgba,
    }
}

fn render_default_case(gpu: &mut ProbeGpu, font_size: f32) -> RenderCase {
    let mut terminal = Terminal::new(ParleyBackend::new(COLUMNS, ROWS)).expect("terminal");
    let mut terminal_renderer =
        TerminalRenderer::new(example_font_options(font_size), Theme::default());
    terminal
        .draw(|frame| ProbePattern { font_size }.render(frame.area(), frame.buffer_mut()))
        .expect("draw probe pattern");

    let buffer = terminal.backend().buffer();
    let (width, height) = terminal_renderer.texture_size_for_buffer(buffer);
    gpu.resize(width, height);
    let cursor_position = terminal.backend().cursor_position();
    gpu.default_renderer
        .render_to_rgba8_into(
            &mut terminal_renderer,
            &mut gpu.readback,
            &gpu.device,
            &gpu.queue,
            &gpu.target,
            buffer,
            Some(cursor_position),
            false,
            &mut gpu.rgba,
        )
        .expect("render default path");
    let rgba = gpu.rgba.clone();
    let stats = analyze_image(&rgba, width, height);
    let metrics = terminal_renderer.metrics();

    RenderCase {
        font_size: font_size as u32,
        width,
        height,
        cell_width: metrics.cell_width,
        cell_height: metrics.cell_height,
        stats,
        rgba,
    }
}

struct ProbePattern {
    font_size: f32,
}

impl Widget for ProbePattern {
    fn render(
        self,
        area: parley_ratatui::ratatui::layout::Rect,
        buf: &mut parley_ratatui::ratatui::buffer::Buffer,
    ) {
        let text = vec![
            Line::from(vec![
                Span::styled(
                    format!("font {:.0}px  ", self.font_size),
                    Style::new()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("HMWNII 0123456789", Style::new().fg(Color::White)),
            ]),
            Line::from("MMMMMMMMMMMM WWWWWWWWWW iiiiiiiiii llllllllll"),
            Line::from("╭────╮ ┌─┬─┐ ██████████ ░▒▓░▒▓░▒▓ --> <--"),
            Line::from("The quick brown fox jumps over the lazy dog."),
            Line::from("AVAVAV ToToTo ffi fl fj []{}() /\\/\\/\\"),
            Line::from("日本語 한글 العربية Ελληνικά кириллица"),
            Line::from("    ← ↑ → ↓ √ ∞ ≈ ≤ ≥ != == ==="),
        ];
        Paragraph::new(text)
            .style(Style::new().fg(Color::White).bg(Color::Black))
            .block(Block::new().borders(Borders::ALL))
            .render(area, buf);
    }
}

fn analyze_image(rgba: &[u8], width: u32, height: u32) -> ImageStats {
    let mut histogram = [0u32; 256];
    let mut covered = 0u32;
    let mut midtone = 0u32;

    for pixel in rgba.chunks_exact(4) {
        let luma = pixel[0];
        histogram[luma as usize] += 1;
        if luma > 0 {
            covered += 1;
        }
        if (1..=254).contains(&luma) {
            midtone += 1;
        }
    }

    let mut edge_count = 0u32;
    let mut hard_edges = 0u32;
    for y in 0..height {
        for x in 0..width {
            let here = luma_at(rgba, width, x, y);
            if x + 1 < width {
                let diff = here.abs_diff(luma_at(rgba, width, x + 1, y));
                if diff >= 8 {
                    edge_count += 1;
                }
                if diff >= 160 {
                    hard_edges += 1;
                }
            }
            if y + 1 < height {
                let diff = here.abs_diff(luma_at(rgba, width, x, y + 1));
                if diff >= 8 {
                    edge_count += 1;
                }
                if diff >= 160 {
                    hard_edges += 1;
                }
            }
        }
    }

    let pixels = width * height;
    ImageStats {
        coverage: covered as f32 / pixels as f32,
        edge_density: edge_count as f32 / pixels as f32,
        hard_edge_ratio: hard_edges as f32 / edge_count.max(1) as f32,
        midtone_ratio: midtone as f32 / covered.max(1) as f32,
        gray_levels: histogram.iter().filter(|&&count| count > 0).count(),
    }
}

fn luma_at(rgba: &[u8], width: u32, x: u32, y: u32) -> u8 {
    rgba[((y * width + x) * 4) as usize]
}

fn print_report(rows: &[Comparison]) {
    println!(
        "font aa    texture   cell      coverage edge_density hard_edge midtone gray_levels penalty"
    );
    for row in rows {
        print_case("msaa8", &row.msaa, row.msaa_penalty);
        print_case("area ", &row.area, row.msaa_penalty);
        print_case("deflt", &row.default, row.msaa_penalty);
        println!(
            "     default-vs-area delta {:.6} (gray levels default={}, area={})",
            row.default_area_delta, row.default.stats.gray_levels, row.area.stats.gray_levels
        );
    }
}

fn print_case(label: &str, case: &RenderCase, penalty: f32) {
    println!(
        "{:>4} {label} {:>4}x{:<4} {:>4.1}x{:<4.1} {:>8.4} {:>12.4} {:>9.4} {:>7.4} {:>11} {:>7.4}",
        case.font_size,
        case.width,
        case.height,
        case.cell_width,
        case.cell_height,
        case.stats.coverage,
        case.stats.edge_density,
        case.stats.hard_edge_ratio,
        case.stats.midtone_ratio,
        case.stats.gray_levels,
        penalty,
    );
}

fn write_snapshots(rows: &[Comparison], count: usize) -> io::Result<()> {
    fs::create_dir_all(OUTPUT_DIR)?;
    for row in rows.iter().take(count) {
        write_bmp(
            Path::new(OUTPUT_DIR).join(format!("font_{:02}_msaa8.bmp", row.font_size)),
            row.msaa.width,
            row.msaa.height,
            &row.msaa.rgba,
        )?;
        write_bmp(
            Path::new(OUTPUT_DIR).join(format!("font_{:02}_area.bmp", row.font_size)),
            row.area.width,
            row.area.height,
            &row.area.rgba,
        )?;
        write_bmp(
            Path::new(OUTPUT_DIR).join(format!("font_{:02}_default.bmp", row.font_size)),
            row.default.width,
            row.default.height,
            &row.default.rgba,
        )?;
    }
    println!("wrote snapshots to {OUTPUT_DIR}");
    Ok(())
}

fn write_bmp(path: impl AsRef<Path>, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
    let row_stride = (width * 3).div_ceil(4) * 4;
    let pixel_size = row_stride * height;
    let file_size = 54 + pixel_size;
    let mut file = File::create(path)?;

    file.write_all(b"BM")?;
    file.write_all(&file_size.to_le_bytes())?;
    file.write_all(&[0; 4])?;
    file.write_all(&54u32.to_le_bytes())?;
    file.write_all(&40u32.to_le_bytes())?;
    file.write_all(&(width as i32).to_le_bytes())?;
    file.write_all(&(height as i32).to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&24u16.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&pixel_size.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;

    let padding = vec![0; (row_stride - width * 3) as usize];
    for y in (0..height).rev() {
        for x in 0..width {
            let index = ((y * width + x) * 4) as usize;
            file.write_all(&[rgba[index + 2], rgba[index + 1], rgba[index]])?;
        }
        file.write_all(&padding)?;
    }

    Ok(())
}

fn example_font_options(size: f32) -> FontOptions {
    const TERMINAL_FAMILIES: &str = "Menlo, JetBrains Mono, FiraMono Nerd Font";

    FontOptions {
        size,
        ..FontOptions::default()
    }
    .with_regular_font(TERMINAL_FAMILIES)
    .with_bold_font(TERMINAL_FAMILIES)
    .with_italic_font(TERMINAL_FAMILIES)
    .with_bold_italic_font(TERMINAL_FAMILIES)
    .with_fallback_family("Apple Color Emoji, Noto Color Emoji")
    .with_fallback_family("Noto Sans CJK JP, PingFang SC, Hiragino Sans")
}
