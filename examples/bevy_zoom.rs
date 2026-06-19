use bevy::camera::visibility::NoFrustumCulling;
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowResized};
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::layout::{Alignment, Constraint, Layout, Rect};
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget};
use parley_ratatui::{
    CellQuantization, FontOptions, ParleyBackend, TerminalRenderer, TexturePresentation, Theme,
};

#[path = "support/bevy_direct.rs"]
mod bevy_direct;

const INITIAL_FONT_SIZE: f32 = 18.0;
const MIN_FONT_SIZE: f32 = 8.0;
const MAX_FONT_SIZE: f32 = 48.0;
const FONT_STEP: f32 = 1.0;

#[derive(Component)]
struct TerminalSprite;

struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    handle: Handle<Image>,
    font_size: f32,
    frame_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalGrid {
    columns: u16,
    rows: u16,
    texture_width: u32,
    texture_height: u32,
}

#[derive(Debug, Clone, Copy)]
struct ScaleDiagnostics {
    scale_factor: f32,
    base_scale_factor: f32,
    render_scale: f32,
    logical_size: Vec2,
    physical_size: UVec2,
    texture_size: UVec2,
    sprite_size: Vec2,
    cell_size: Vec2,
    expected_physical_size: Vec2,
    delta: Vec2,
}

impl ScaleDiagnostics {
    fn new(window: &Window, grid: TerminalGrid, cell_size: Vec2, render_scale: f32) -> Self {
        let scale_factor = window.scale_factor();
        let presentation =
            TexturePresentation::new([grid.texture_width, grid.texture_height], render_scale);
        let [sprite_width, sprite_height] = presentation.logical_size();
        let [expected_width, expected_height] = presentation.expected_physical_size();
        let [delta_x, delta_y] = presentation.physical_delta();
        let sprite_size = Vec2::new(sprite_width, sprite_height);
        let expected_physical_size = Vec2::new(expected_width, expected_height);
        let texture_size = UVec2::new(grid.texture_width, grid.texture_height);
        let delta = Vec2::new(delta_x, delta_y);

        Self {
            scale_factor,
            base_scale_factor: window.resolution.base_scale_factor(),
            render_scale,
            logical_size: window.resolution.size(),
            physical_size: window.resolution.physical_size(),
            texture_size,
            sprite_size,
            cell_size,
            expected_physical_size,
            delta,
        }
    }

    fn is_exact(self) -> bool {
        self.delta.x.abs() <= 0.01 && self.delta.y.abs() <= 0.01
    }
}

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(ImagePlugin::default_linear()),
            bevy_direct::DirectTerminalPlugin,
        ))
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
    let render_scale = bevy_direct::render_scale_for_window(primary_window);
    let logical_size = primary_window.resolution.size();

    let mut terminal = Terminal::new(ParleyBackend::new(1, 1)).expect("terminal");
    let renderer = TerminalRenderer::new_scaled(
        example_font_options(INITIAL_FONT_SIZE),
        Theme::default(),
        render_scale,
    );
    let grid = resize_terminal_to_fit(&mut terminal, &renderer, logical_size, render_scale);
    let image = bevy_direct::new_terminal_image(
        grid.texture_width,
        grid.texture_height,
        "parley_ratatui.bevy_zoom",
    );
    let handle = world.resource_mut::<Assets<Image>>().add(image);

    world.spawn(Camera2d);
    // Present the terminal texture with a fullscreen quad that fetches each texel
    // by physical pixel coordinate (1:1, no resampling), rather than a sprite.
    let present_mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(bevy_direct::present_quad());
    let present_material = world
        .resource_mut::<Assets<bevy_direct::TerminalPresentMaterial>>()
        .add(bevy_direct::TerminalPresentMaterial {
            texture: handle.clone(),
        });
    world.spawn((
        Mesh2d(present_mesh),
        MeshMaterial2d(present_material),
        NoFrustumCulling,
        TerminalSprite,
    ));

    world.insert_non_send(TerminalTexture {
        terminal,
        renderer,
        handle,
        font_size: INITIAL_FONT_SIZE,
        frame_count: 0,
    });
}

fn keyboard_zoom(
    keyboard: Res<ButtonInput<KeyCode>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
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
    let window = windows.single().expect("primary window");
    let render_scale = bevy_direct::render_scale_for_window(window);
    terminal_texture.renderer = TerminalRenderer::new_scaled(
        example_font_options(font_size),
        Theme::default(),
        render_scale,
    );

    let logical_size = window.resolution.size();
    let _ = resize_terminal_to_fit(
        &mut terminal_texture.terminal,
        &terminal_texture.renderer,
        logical_size,
        render_scale,
    );
}

fn resize_terminal(
    mut events: MessageReader<WindowResized>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
) {
    if events.read().next().is_none() {
        return;
    }

    let window = windows.single().expect("primary window");
    let logical_size = window.resolution.size();
    let render_scale = bevy_direct::render_scale_for_window(window);
    let terminal_texture = &mut *terminal_texture;
    terminal_texture.renderer = TerminalRenderer::new_scaled(
        example_font_options(terminal_texture.font_size),
        Theme::default(),
        render_scale,
    );
    let _ = resize_terminal_to_fit(
        &mut terminal_texture.terminal,
        &terminal_texture.renderer,
        logical_size,
        render_scale,
    );
}

fn update_terminal_texture(
    exchange: Res<bevy_direct::DirectTerminalSceneExchange>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
    mut present_materials: ResMut<Assets<bevy_direct::TerminalPresentMaterial>>,
    present_query: Query<&MeshMaterial2d<bevy_direct::TerminalPresentMaterial>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    let window = windows.single().expect("primary window");

    let TerminalTexture {
        terminal,
        renderer,
        handle,
        font_size,
        frame_count,
    } = &mut *terminal_texture;

    let area = terminal.backend().buffer().area;
    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
    let metrics = renderer.metrics();
    let render_scale = bevy_direct::render_scale_for_window(window);
    let grid = TerminalGrid {
        columns: area.width,
        rows: area.height,
        texture_width: width,
        texture_height: height,
    };
    let diagnostics = ScaleDiagnostics::new(
        window,
        grid,
        Vec2::new(metrics.cell_width, metrics.cell_height).max(Vec2::ONE) / render_scale,
        render_scale,
    );

    terminal
        .draw(|frame| {
            TerminalDemo {
                font_size: *font_size,
                columns: area.width,
                rows: area.height,
                frame_count: *frame_count,
                elapsed: time.elapsed_secs(),
                diagnostics,
            }
            .render(frame.area(), frame.buffer_mut());
        })
        .expect("draw terminal");
    *frame_count = frame_count.wrapping_add(1);

    let mut image = images.get_mut(&*handle).expect("terminal image");
    bevy_direct::resize_terminal_image(&mut image, width, height);

    // The texture's GpuImage is recreated when it resizes (font zoom / window
    // resize), which invalidates the present material's cached bind group. Writing
    // the texture handle (not merely touching the asset) advances the material's
    // change tick so Bevy re-prepares the bind group against the current GpuImage;
    // a no-op `get_mut` is not enough and leaves the quad sampling a stale texture.
    for present in &present_query {
        if let Some(mut material) = present_materials.get_mut(&present.0) {
            material.texture = handle.clone();
        }
    }

    let cursor_position = terminal.backend().cursor_position();
    let cursor_visible = terminal.backend().cursor_visible();
    let buffer = terminal.backend().buffer();
    bevy_direct::update_direct_terminal_frame(
        &exchange,
        handle.clone(),
        renderer,
        buffer,
        Some(cursor_position),
        cursor_visible,
        time.elapsed_secs(),
    );
}

fn resize_terminal_to_fit(
    terminal: &mut Terminal<ParleyBackend>,
    renderer: &TerminalRenderer,
    logical_size: Vec2,
    render_scale: f32,
) -> TerminalGrid {
    let metrics = renderer.logical_metrics(render_scale);
    let logical_size = logical_size.max(Vec2::ONE);
    let columns = (logical_size.x / metrics.cell_width)
        .floor()
        .clamp(1.0, u16::MAX as f32) as u16;
    let rows = (logical_size.y / metrics.cell_height)
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
    // Keep exact fractional cell sizes so both axes scale proportionally on every
    // font-size step. With the default `Round`, a sub-pixel advance delta can leave
    // the cell width unchanged while the height grows — a vertical-only stretch that
    // is especially visible on low-DPI monitors (small physical cells round harder).
    .with_cell_quantization(CellQuantization::Fractional)
}

struct TerminalDemo {
    font_size: f32,
    columns: u16,
    rows: u16,
    frame_count: u64,
    elapsed: f32,
    diagnostics: ScaleDiagnostics,
}

impl Widget for TerminalDemo {
    fn render(self, area: Rect, buf: &mut parley_ratatui::ratatui::buffer::Buffer) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .areas(area);

        let title = format!(
            " Bevy zoom example | font {:.1}px | scale {:.2} | {}x{} ",
            self.font_size, self.diagnostics.render_scale, self.columns, self.rows
        );
        let status_style = if self.diagnostics.is_exact() {
            Style::new().fg(Color::LightGreen).bold()
        } else {
            Style::new().fg(Color::LightRed).bold()
        };
        let status_text = if self.diagnostics.is_exact() {
            "1:1 physical pixels"
        } else {
            "resampled"
        };
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
            Line::from(vec![
                Span::styled("Window logical ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{:.1}x{:.1}",
                        self.diagnostics.logical_size.x, self.diagnostics.logical_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
                Span::styled(" physical ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{}x{}",
                        self.diagnostics.physical_size.x, self.diagnostics.physical_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Scale reported ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!("{:.3}", self.diagnostics.scale_factor),
                    Style::new().fg(Color::White),
                ),
                Span::styled(" base ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!("{:.3}", self.diagnostics.base_scale_factor),
                    Style::new().fg(Color::White),
                ),
                Span::styled(" render ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!("{:.3}", self.diagnostics.render_scale),
                    Style::new().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Texture ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{}x{}",
                        self.diagnostics.texture_size.x, self.diagnostics.texture_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
                Span::styled(" cell ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{:.2}x{:.2}",
                        self.diagnostics.cell_size.x, self.diagnostics.cell_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled(" sprite ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{:.2}x{:.2}",
                        self.diagnostics.sprite_size.x, self.diagnostics.sprite_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
                Span::styled(" expected physical ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{:.2}x{:.2}",
                        self.diagnostics.expected_physical_size.x,
                        self.diagnostics.expected_physical_size.y
                    ),
                    Style::new().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Display ", Style::new().fg(Color::Gray)),
                Span::styled(status_text, status_style),
                Span::styled(" delta ", Style::new().fg(Color::Gray)),
                Span::styled(
                    format!(
                        "{:+.3},{:+.3}px",
                        self.diagnostics.delta.x, self.diagnostics.delta.y
                    ),
                    status_style,
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
