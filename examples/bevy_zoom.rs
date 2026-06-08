use bevy::asset::RenderAssetUsages;
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat as BevyTextureFormat,
};
use bevy::window::{PrimaryWindow, WindowResized};
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::layout::{Alignment, Constraint, Layout, Rect};
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget};
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureReadback, TextureTarget,
    Theme,
};

const INITIAL_FONT_SIZE: f32 = 18.0;
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 48.0;
const FONT_STEP: f32 = 1.0;

#[derive(Component)]
struct TerminalSprite;

struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    gpu: OffscreenGpu,
    handle: Handle<Image>,
    font_size: f32,
    frame_count: u64,
}

struct OffscreenGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: GpuRenderer,
    target: TextureTarget,
    readback: TextureReadback,
    rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalGrid {
    columns: u16,
    rows: u16,
    texture_width: u32,
    texture_height: u32,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (keyboard_zoom, resize_terminal, update_terminal_texture).chain(),
        )
        .run();
}

fn setup(world: &mut World) {
    let primary_window = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(world)
        .expect("primary window");
    let scale_factor = primary_window.scale_factor();
    let logical_size = primary_window.resolution.size();

    let mut terminal = Terminal::new(ParleyBackend::new(1, 1)).expect("terminal");
    let renderer = TerminalRenderer::new(example_font_options(INITIAL_FONT_SIZE), Theme::default());
    let grid = resize_terminal_to_fit(&mut terminal, &renderer, logical_size, scale_factor);
    let gpu = pollster::block_on(OffscreenGpu::new(grid.texture_width, grid.texture_height));

    let image = Image::new_fill(
        Extent3d {
            width: grid.texture_width,
            height: grid.texture_height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[15, 18, 24, 255],
        BevyTextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = world.resource_mut::<Assets<Image>>().add(image);

    world.spawn(Camera2d);
    world.spawn((
        Sprite {
            image: handle.clone(),
            custom_size: Some(texture_logical_size(grid, scale_factor)),
            ..default()
        },
        Transform::from_translation(Vec3::ZERO),
        TerminalSprite,
    ));

    world.insert_non_send_resource(TerminalTexture {
        terminal,
        renderer,
        gpu,
        handle,
        font_size: INITIAL_FONT_SIZE,
        frame_count: 0,
    });
}

fn keyboard_zoom(
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut sprite: Query<&mut Sprite, With<TerminalSprite>>,
) {
    let zoom_in =
        keyboard.just_pressed(KeyCode::Equal) || keyboard.just_pressed(KeyCode::NumpadAdd);
    let zoom_out =
        keyboard.just_pressed(KeyCode::Minus) || keyboard.just_pressed(KeyCode::NumpadSubtract);
    if !zoom_in && !zoom_out {
        return;
    }

    let terminal_texture = &mut *terminal_texture;
    let delta = if zoom_in { FONT_STEP } else { -FONT_STEP };
    let font_size = (terminal_texture.font_size + delta).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
    if (font_size - terminal_texture.font_size).abs() < f32::EPSILON {
        return;
    }

    terminal_texture.font_size = font_size;
    terminal_texture.renderer =
        TerminalRenderer::new(example_font_options(font_size), Theme::default());

    let window = windows.single().expect("primary window");
    let logical_size = window.resolution.size();
    let scale_factor = window.scale_factor();
    let grid = resize_terminal_to_fit(
        &mut terminal_texture.terminal,
        &terminal_texture.renderer,
        logical_size,
        scale_factor,
    );
    sync_sprite_size(&mut sprite, grid, scale_factor);
}

fn resize_terminal(
    mut events: MessageReader<WindowResized>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut sprite: Query<&mut Sprite, With<TerminalSprite>>,
) {
    if events.read().next().is_none() {
        return;
    }

    let window = windows.single().expect("primary window");
    let logical_size = window.resolution.size();
    let scale_factor = window.scale_factor();
    let terminal_texture = &mut *terminal_texture;
    let grid = resize_terminal_to_fit(
        &mut terminal_texture.terminal,
        &terminal_texture.renderer,
        logical_size,
        scale_factor,
    );
    sync_sprite_size(&mut sprite, grid, scale_factor);
}

fn update_terminal_texture(
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    let window = windows.single().expect("primary window");
    let scale_factor = window.scale_factor();

    let TerminalTexture {
        terminal,
        renderer,
        gpu,
        handle,
        font_size,
        frame_count,
    } = &mut *terminal_texture;

    let area = terminal.backend().buffer().area;
    terminal
        .draw(|frame| {
            TerminalDemo {
                font_size: *font_size,
                scale_factor,
                columns: area.width,
                rows: area.height,
                frame_count: *frame_count,
                elapsed: time.elapsed_secs(),
            }
            .render(frame.area(), frame.buffer_mut());
        })
        .expect("draw terminal");
    *frame_count = frame_count.wrapping_add(1);

    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
    let image = images.get_mut(&*handle).expect("terminal image");
    gpu.resize(width, height);

    let cursor_position = terminal.backend().cursor_position();
    let cursor_visible = terminal.backend().cursor_visible();
    let buffer = terminal.backend().buffer();
    gpu.renderer
        .render_to_rgba8_into(
            renderer,
            &mut gpu.readback,
            &gpu.device,
            &gpu.queue,
            &gpu.target,
            buffer,
            Some(cursor_position),
            cursor_visible,
            &mut gpu.rgba,
        )
        .expect("render terminal texture");

    if image.texture_descriptor.size.width != width
        || image.texture_descriptor.size.height != height
    {
        image.resize(Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        });
    }
    let data = image.data.get_or_insert_with(Vec::new);
    if data.len() != gpu.rgba.len() {
        data.resize(gpu.rgba.len(), 0);
    }
    data.copy_from_slice(&gpu.rgba);
}

fn resize_terminal_to_fit(
    terminal: &mut Terminal<ParleyBackend>,
    renderer: &TerminalRenderer,
    logical_size: Vec2,
    scale_factor: f32,
) -> TerminalGrid {
    let metrics = renderer.metrics();
    let physical_size = logical_size.max(Vec2::ONE) * scale_factor.max(1.0);
    let columns = (physical_size.x / metrics.cell_width)
        .floor()
        .clamp(1.0, u16::MAX as f32) as u16;
    let rows = (physical_size.y / metrics.cell_height)
        .floor()
        .clamp(1.0, u16::MAX as f32) as u16;

    let current_area = terminal.backend().buffer().area;
    if current_area.width != columns || current_area.height != rows {
        terminal.backend_mut().resize(columns, rows);
    }

    let (texture_width, texture_height) =
        renderer.texture_size_for_buffer(terminal.backend().buffer());
    TerminalGrid {
        columns,
        rows,
        texture_width,
        texture_height,
    }
}

fn sync_sprite_size(
    sprite: &mut Query<&mut Sprite, With<TerminalSprite>>,
    grid: TerminalGrid,
    scale_factor: f32,
) {
    let mut sprite = sprite.single_mut().expect("terminal sprite");
    sprite.custom_size = Some(texture_logical_size(grid, scale_factor));
}

fn texture_logical_size(grid: TerminalGrid, scale_factor: f32) -> Vec2 {
    Vec2::new(
        grid.texture_width as f32 / scale_factor.max(1.0),
        grid.texture_height as f32 / scale_factor.max(1.0),
    )
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
            Some("parley_ratatui.bevy_zoom"),
        );
        let renderer = GpuRenderer::new(&device).expect("vello renderer");
        let readback = TextureReadback::new();

        Self {
            device,
            queue,
            renderer,
            target,
            readback,
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
            Some("parley_ratatui.bevy_zoom"),
        );
        self.readback = TextureReadback::new();
        self.rgba.clear();
    }
}

struct TerminalDemo {
    font_size: f32,
    scale_factor: f32,
    columns: u16,
    rows: u16,
    frame_count: u64,
    elapsed: f32,
}

impl Widget for TerminalDemo {
    fn render(self, area: Rect, buf: &mut parley_ratatui::ratatui::buffer::Buffer) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .areas(area);

        let title = format!(
            " Bevy zoom example | font {:.1}px | scale {:.2} | {}x{} ",
            self.font_size, self.scale_factor, self.columns, self.rows
        );
        let header_text = vec![
            Line::from(vec![
                Span::styled("Zoom: ", Style::new().fg(Color::Gray)),
                Span::styled("+", Style::new().fg(Color::LightGreen).bold()),
                Span::raw(" / "),
                Span::styled("-", Style::new().fg(Color::LightRed).bold()),
                Span::raw("  "),
                Span::styled("Resize the window", Style::new().fg(Color::LightCyan)),
                Span::raw("  "),
                Span::styled(
                    "texture is never stretched",
                    Style::new().fg(Color::LightYellow).italic(),
                ),
            ]),
            Line::from(vec![
                Span::styled("Frame ", Style::new().fg(Color::Gray)),
                Span::styled(self.frame_count.to_string(), Style::new().fg(Color::White)),
                Span::raw("  "),
                Span::styled("Elapsed ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!("{:.2}s", self.elapsed),
                    Style::new().fg(Color::White),
                ),
            ]),
        ];
        Paragraph::new(header_text)
            .block(Block::new().title(title).borders(Borders::ALL))
            .render(header, buf);

        let [left, right] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                .areas(body);
        demo_table().render(left, buf);
        Paragraph::new(demo_lines(self.elapsed))
            .block(
                Block::new()
                    .title(" Glyphs and styles ")
                    .borders(Borders::ALL),
            )
            .render(right, buf);

        Paragraph::new(Line::from(vec![
            Span::styled("Invariant: ", Style::new().fg(Color::Gray)),
            Span::styled(
                "font size changes metrics, metrics change rows/columns, rows/columns change texture size, sprite displays that texture at exact logical size",
                Style::new().fg(Color::LightGreen),
            ),
        ]))
        .alignment(Alignment::Center)
        .block(Block::new().borders(Borders::ALL))
        .render(footer, buf);
    }
}

fn demo_table() -> Table<'static> {
    let rows = [
        ("Normal", "ASCII text", Color::White),
        ("Bold", "heavy weight", Color::LightCyan),
        ("Italic", "slanted style", Color::LightMagenta),
        ("Unicode", "日本語 한글 العربية", Color::LightYellow),
        ("Symbols", "  ← ↑ → ↓ √ ∞", Color::LightGreen),
        ("Blocks", "█ ▉ ▊ ▋ ▌ ▍ ▎ ▏", Color::LightBlue),
    ]
    .into_iter()
    .map(|(name, value, color)| {
        Row::new([
            Cell::from(name),
            Cell::from(Span::styled(value, Style::new().fg(color))),
        ])
    });

    Table::new(rows, [Constraint::Length(12), Constraint::Min(20)])
        .header(Row::new(["Case", "Sample"]).style(Style::new().add_modifier(Modifier::BOLD)))
        .block(Block::new().title(" Stable cells ").borders(Borders::ALL))
}

fn demo_lines(elapsed: f32) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("modifiers ", Style::new().fg(Color::Gray)),
            Span::styled("BOLD", Style::new().fg(Color::White).bold()),
            Span::raw("  "),
            Span::styled("DIM", Style::new().fg(Color::White).dim()),
            Span::raw("  "),
            Span::styled("ITALIC", Style::new().fg(Color::White).italic()),
            Span::raw("  "),
            Span::styled("UNDERLINED", Style::new().fg(Color::White).underlined()),
        ]),
        Line::from(vec![
            Span::styled("palette   ", Style::new().fg(Color::Gray)),
            swatch("Red", Color::Red),
            swatch("Green", Color::Green),
            swatch("Yellow", Color::Yellow),
            swatch("Blue", Color::Blue),
            swatch("Magenta", Color::Magenta),
            swatch("Cyan", Color::Cyan),
        ]),
        Line::from(vec![
            Span::styled("emoji     ", Style::new().fg(Color::Gray)),
            Span::raw("😀 🚀 ✨ 🔥 👩‍💻 🧑🏽‍🚀"),
        ]),
        Line::from(vec![
            Span::styled("box       ", Style::new().fg(Color::Gray)),
            Span::styled("┌─┬─┐ ├─┼─┤ └─┴─┘", Style::new().fg(Color::LightCyan)),
            Span::raw("  "),
            Span::styled("░ ▒ ▓", Style::new().fg(Color::LightYellow)),
        ]),
        Line::from(vec![
            Span::styled("animated  ", Style::new().fg(Color::Gray)),
            Span::styled(
                format!("phase {:.2}", elapsed),
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
