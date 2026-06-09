use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph};
use parley_ratatui::{FontOptions, ParleyBackend, PresentationScale, TerminalRenderer, Theme};

#[path = "support/bevy_direct.rs"]
mod bevy_direct;

struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    handle: Handle<Image>,
}

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins.set(ImagePlugin::default_linear()),
            bevy_direct::DirectTerminalPlugin,
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, update_terminal_texture)
        .run();
}

fn setup(world: &mut World) {
    let primary_window = world
        .query_filtered::<&Window, With<PrimaryWindow>>()
        .single(world)
        .expect("primary window");
    let render_scale = render_scale_for_window(primary_window);
    let terminal = Terminal::new(ParleyBackend::new(94, 31)).expect("terminal");
    let renderer =
        TerminalRenderer::new_scaled(example_font_options(), Theme::default(), render_scale);
    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
    let image = bevy_direct::new_terminal_image(width, height, "parley_ratatui.bevy_texture");
    let handle = world.resource_mut::<Assets<Image>>().add(image);

    world.spawn(Camera2d);
    let sprite_position = snapped_translation(Vec2::ZERO, render_scale);
    world
        .spawn(Sprite {
            image: handle.clone(),
            custom_size: Some(bevy_direct::texture_logical_size(
                width,
                height,
                render_scale,
            )),
            ..default()
        })
        .insert(Transform::from_translation(sprite_position.extend(0.0)));

    world.insert_non_send(TerminalTexture {
        terminal,
        renderer,
        handle,
    });
}

fn render_scale_for_window(window: &Window) -> f32 {
    let logical_size = window.resolution.size().max(Vec2::ONE);
    let physical_size = window.resolution.physical_size();
    PresentationScale::new(
        [logical_size.x, logical_size.y],
        [physical_size.x, physical_size.y],
        window.scale_factor(),
        window.resolution.base_scale_factor(),
    )
    .render_scale()
}

fn snapped_translation(position: Vec2, render_scale: f32) -> Vec2 {
    let [x, y] = parley_ratatui::snap_logical_position_to_physical_pixel(
        [position.x, position.y],
        render_scale,
    );
    Vec2::new(x, y)
}

fn example_font_options() -> FontOptions {
    const TERMINAL_FAMILIES: &str = "Menlo, JetBrains Mono, FiraMono Nerd Font";

    FontOptions::default()
        .with_regular_font(TERMINAL_FAMILIES)
        .with_bold_font(TERMINAL_FAMILIES)
        .with_italic_font(TERMINAL_FAMILIES)
        .with_bold_italic_font(TERMINAL_FAMILIES)
        .with_fallback_family("Apple Color Emoji, Noto Color Emoji")
        .with_fallback_family("Noto Sans CJK JP, PingFang SC, Hiragino Sans")
}

fn update_terminal_texture(
    exchange: Res<bevy_direct::DirectTerminalSceneExchange>,
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time>,
) {
    let TerminalTexture {
        terminal,
        renderer,
        handle,
    } = &mut *terminal_texture;
    terminal
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

    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
    let mut image = images.get_mut(&*handle).expect("terminal image");
    bevy_direct::resize_terminal_image(&mut image, width, height);

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
        Line::from(vec![
            Span::styled("chess      ", Style::new().fg(Color::Gray)),
            Span::raw("♝ ♖ ♞ ♙ ♚ "),
            Span::styled("Chess Piece Symbols", Style::new().fg(Color::LightYellow)),
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
