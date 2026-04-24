use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat as BevyTextureFormat,
};
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph};
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureTarget, Theme,
};

struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    gpu: OffscreenGpu,
    handle: Handle<Image>,
}

struct OffscreenGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: GpuRenderer,
    target: TextureTarget,
}

impl OffscreenGpu {
    async fn new(width: u32, height: u32) -> Self {
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .expect("wgpu adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("wgpu device");
        let target = TextureTarget::new(
            &device,
            width,
            height,
            wgpu::TextureFormat::Rgba8Unorm,
            Some("parley_ratatui.bevy_texture"),
        );
        let renderer = GpuRenderer::new(&device).expect("vello renderer");

        Self {
            device,
            queue,
            renderer,
            target,
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
            Some("parley_ratatui.bevy_texture"),
        );
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_systems(Startup, setup)
        .add_systems(Update, update_terminal_texture)
        .run();
}

fn setup(world: &mut World) {
    let terminal = Terminal::new(ParleyBackend::new(94, 31)).expect("terminal");
    let renderer = TerminalRenderer::new(FontOptions::default(), Theme::default());
    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
    let gpu = pollster::block_on(OffscreenGpu::new(width, height));

    let image = Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[17, 24, 39, 255],
        BevyTextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = world.resource_mut::<Assets<Image>>().add(image);

    world.spawn(Camera2d);
    world.spawn(Sprite {
        image: handle.clone(),
        custom_size: Some(Vec2::new(width as f32, height as f32)),
        ..default()
    });

    world.insert_non_send_resource(TerminalTexture {
        terminal,
        renderer,
        gpu,
        handle,
    });
}

fn update_terminal_texture(
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time>,
) {
    let terminal_texture = &mut *terminal_texture;
    terminal_texture
        .terminal
        .draw(|frame| {
            let area = frame.area();
            let title = format!(
                "Parley Ratatui -> Bevy Texture  {:.2}s  unicode/style matrix",
                time.elapsed_secs()
            );
            let paragraph = Paragraph::new(demo_lines(time.elapsed_secs()))
                .block(Block::new().title(title).borders(Borders::ALL));
            frame.render_widget(paragraph, area);
        })
        .expect("draw terminal");

    let (width, height) = terminal_texture
        .renderer
        .texture_size_for_buffer(terminal_texture.terminal.backend().buffer());
    let image = images
        .get_mut(&terminal_texture.handle)
        .expect("terminal image");
    if image.texture_descriptor.size.width != width
        || image.texture_descriptor.size.height != height
    {
        image.resize(Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        });
    }
    terminal_texture.gpu.resize(width, height);

    let buffer = terminal_texture.terminal.backend().buffer().clone();
    let cursor_position = terminal_texture.terminal.backend().cursor_position();
    let cursor_visible = terminal_texture.terminal.backend().cursor_visible();
    let TerminalTexture { renderer, gpu, .. } = &mut *terminal_texture;
    let OffscreenGpu {
        device,
        queue,
        renderer: gpu_renderer,
        target,
    } = gpu;
    let rgba = gpu_renderer
        .render_to_rgba8(
            renderer,
            device,
            queue,
            target,
            &buffer,
            Some(cursor_position),
            cursor_visible,
        )
        .expect("render terminal texture");
    image.data = Some(rgba);
}

fn demo_lines(elapsed: f32) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("render path ", Style::new().fg(Color::Gray)),
            Span::styled("Ratatui Buffer", Style::new().fg(Color::LightCyan).bold()),
            Span::raw(" -> "),
            Span::styled(
                "Parley shaping",
                Style::new().fg(Color::LightGreen).italic(),
            ),
            Span::raw(" -> "),
            Span::styled(
                "Vello/wgpu texture",
                Style::new().fg(Color::LightYellow).underlined(),
            ),
            Span::raw(" -> "),
            Span::styled("Bevy Image", Style::new().fg(Color::LightMagenta).bold()),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("modifiers ", Style::new().fg(Color::Gray)),
            Span::styled("BOLD", Style::new().fg(Color::White).bold()),
            Span::raw("  "),
            Span::styled("DIM", Style::new().fg(Color::White).dim()),
            Span::raw("  "),
            Span::styled("ITALIC", Style::new().fg(Color::White).italic()),
            Span::raw("  "),
            Span::styled("UNDERLINED", Style::new().fg(Color::White).underlined()),
            Span::raw("  "),
            Span::styled("CROSSED_OUT", Style::new().fg(Color::White).crossed_out()),
        ]),
        Line::from(vec![
            Span::styled("modifiers ", Style::new().fg(Color::Gray)),
            Span::styled("SLOW_BLINK", Style::new().fg(Color::LightRed).slow_blink()),
            Span::raw("  "),
            Span::styled(
                "RAPID_BLINK",
                Style::new().fg(Color::LightYellow).rapid_blink(),
            ),
            Span::raw("  "),
            Span::styled(
                "REVERSED",
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .reversed(),
            ),
            Span::raw("  "),
            Span::styled("HIDDEN:", Style::new().fg(Color::Gray)),
            Span::styled("invisible text", Style::new().fg(Color::LightRed).hidden()),
            Span::raw("  "),
            Span::styled(
                "ALL FLAGS",
                Style::new().fg(Color::LightGreen).add_modifier(
                    Modifier::BOLD
                        | Modifier::DIM
                        | Modifier::ITALIC
                        | Modifier::UNDERLINED
                        | Modifier::SLOW_BLINK
                        | Modifier::RAPID_BLINK
                        | Modifier::REVERSED
                        | Modifier::CROSSED_OUT,
                ),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("ansi palette ", Style::new().fg(Color::Gray)),
            swatch("Black", Color::Black),
            swatch("Red", Color::Red),
            swatch("Green", Color::Green),
            swatch("Yellow", Color::Yellow),
            swatch("Blue", Color::Blue),
            swatch("Magenta", Color::Magenta),
            swatch("Cyan", Color::Cyan),
            swatch("Gray", Color::Gray),
        ]),
        Line::from(vec![
            Span::styled("bright/rgb  ", Style::new().fg(Color::Gray)),
            swatch("LightRed", Color::LightRed),
            swatch("LightGreen", Color::LightGreen),
            swatch("LightBlue", Color::LightBlue),
            Span::styled(" truecolor ", Style::new().fg(Color::Rgb(255, 160, 64))),
            Span::styled(" indexed-202 ", Style::new().fg(Color::Indexed(202))),
            Span::styled(" indexed-51 ", Style::new().fg(Color::Indexed(51))),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("CJK        ", Style::new().fg(Color::Gray)),
            Span::styled("日本語 こんにちは", Style::new().fg(Color::LightCyan)),
            Span::raw("  "),
            Span::styled("简体中文 终端渲染", Style::new().fg(Color::LightGreen)),
            Span::raw("  "),
            Span::styled("繁體中文 字形測試", Style::new().fg(Color::LightYellow)),
        ]),
        Line::from(vec![
            Span::styled("Korean     ", Style::new().fg(Color::Gray)),
            Span::styled("한글 표시 테스트", Style::new().fg(Color::LightMagenta)),
            Span::raw("  "),
            Span::styled("Kana カタカナ ひらがな", Style::new().fg(Color::LightBlue)),
        ]),
        Line::from(vec![
            Span::styled("combining  ", Style::new().fg(Color::Gray)),
            Span::raw("e\u{301} cafe\u{301}  a\u{308} o\u{302} n\u{303}  "),
            Span::styled("Devanagari नमस्ते", Style::new().fg(Color::LightGreen)),
            Span::raw("  "),
            Span::styled("Arabic العربية", Style::new().fg(Color::LightYellow)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("emoji      ", Style::new().fg(Color::Gray)),
            Span::raw("😀 😃 😄 😁 🚀 ✨ 🔥 "),
            Span::styled("color + fallback", Style::new().fg(Color::LightCyan).bold()),
        ]),
        Line::from(vec![
            Span::styled("emoji seq  ", Style::new().fg(Color::Gray)),
            Span::raw("👩‍💻 🧑🏽‍🚀 🏳️‍🌈 ❤️‍🔥 👍🏿  keycaps 1️⃣ 2️⃣ #️⃣"),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("box/block  ", Style::new().fg(Color::Gray)),
            Span::styled("┌─┬─┐ ├─┼─┤ └─┴─┘", Style::new().fg(Color::LightCyan)),
            Span::raw("  "),
            Span::styled("█ ▉ ▊ ▋ ▌ ▍ ▎ ▏", Style::new().fg(Color::LightGreen)),
            Span::raw("  "),
            Span::styled("░ ▒ ▓", Style::new().fg(Color::LightYellow)),
        ]),
        Line::from(vec![
            Span::styled("symbols    ", Style::new().fg(Color::Gray)),
            Span::raw("← ↑ → ↓ ⇐ ⇑ ⇒ ⇓  ≤ ≥ ≠ ≈ ∑ ∫ √ ∞  "),
            Span::styled("Powerline    ", Style::new().fg(Color::LightMagenta)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("background ", Style::new().fg(Color::Gray)),
            Span::styled(" red ", Style::new().fg(Color::White).bg(Color::Red)),
            Span::styled(" green ", Style::new().fg(Color::Black).bg(Color::Green)),
            Span::styled(" blue ", Style::new().fg(Color::White).bg(Color::Blue)),
            Span::styled(
                " rgb ",
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::Rgb(250, 204, 21))
                    .bold(),
            ),
            Span::raw("  "),
            Span::styled(
                format!("animated {:.2}", elapsed),
                Style::new()
                    .fg(Color::Black)
                    .bg(animated_color(elapsed))
                    .bold(),
            ),
        ]),
    ]
}

fn swatch(label: &'static str, color: Color) -> Span<'static> {
    Span::styled(format!(" {label} "), Style::new().fg(color).bold())
}

fn animated_color(elapsed: f32) -> Color {
    let phase = elapsed.sin() * 0.5 + 0.5;
    let r = (64.0 + phase * 191.0) as u8;
    let g = (224.0 - phase * 96.0) as u8;
    let b = (255.0 - phase * 191.0) as u8;
    Color::Rgb(r, g, b)
}
