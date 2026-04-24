use std::sync::Mutex;

use parley_ratatui::{OwnedTextureRenderer, ParleyBackend, PixelSize};
use ratatui::Terminal;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color as RatatuiColor, Modifier, Style};
use ratatui::widgets::{Block, Gauge, Paragraph, Sparkline};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::WindowResolution;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "parley_ratatui Bevy texture example".into(),
                resolution: WindowResolution::new(1000, 640),
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, setup)
        .add_systems(Update, redraw_terminal)
        .run();
}

#[derive(Resource)]
struct TuiTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: Mutex<OwnedTextureRenderer>,
    image: Handle<Image>,
    frame: u64,
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let terminal =
        Terminal::new(ParleyBackend::new(92, 28, 16.0)).expect("failed to create terminal");
    let size = terminal.backend().pixel_size();
    let mut image = new_image(size);
    image.sampler = ImageSampler::nearest();
    let image = images.add(image);

    commands.spawn(Camera2d);
    commands.spawn((
        Sprite::from_image(image.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));

    let renderer = OwnedTextureRenderer::new(size).expect("failed to create renderer");
    commands.insert_resource(TuiTexture {
        terminal,
        renderer: Mutex::new(renderer),
        image,
        frame: 0,
    });
}

fn redraw_terminal(
    mut tui: ResMut<TuiTexture>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time>,
) {
    let elapsed = time.elapsed_secs_f64();
    let (rgba, size, image_handle) = {
        let tui = tui.as_mut();
        tui.frame = tui.frame.wrapping_add(1);
        tui.terminal
            .draw(|frame| render_ui(frame, elapsed, frame.area()))
            .expect("failed to draw terminal");

        let TuiTexture {
            terminal,
            renderer,
            image,
            ..
        } = tui;
        let size = terminal.backend().pixel_size();
        let mut renderer = renderer.lock().expect("renderer mutex poisoned");
        let rgba = renderer
            .render_backend_to_rgba(terminal.backend_mut())
            .expect("failed to render terminal")
            .to_vec();
        (rgba, size, image.clone())
    };

    let image = images
        .get_mut(&image_handle)
        .expect("terminal image handle should exist");
    if image.texture_descriptor.size.width != size.width
        || image.texture_descriptor.size.height != size.height
    {
        *image = new_image(size);
        image.sampler = ImageSampler::nearest();
    }
    image.data = Some(rgba);
}

fn new_image(size: PixelSize) -> Image {
    Image::new_fill(
        Extent3d {
            width: size.width.max(1),
            height: size.height.max(1),
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn render_ui(frame: &mut ratatui::Frame<'_>, elapsed: f64, area: ratatui::layout::Rect) {
    let [header, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(5),
        ])
        .areas(area);

    frame.render_widget(
        Paragraph::new("parley_ratatui rendered into a Bevy Image")
            .block(Block::bordered().title("Bevy"))
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(RatatuiColor::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
        header,
    );

    let sparkline = (0..80)
        .map(|i| {
            let phase = elapsed + i as f64 * 0.18;
            ((phase.sin() * 0.5 + 0.5) * 100.0) as u64
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Sparkline::default()
            .block(Block::bordered().title("Live data"))
            .data(&sparkline)
            .style(RatatuiColor::Green),
        body,
    );

    let percent = ((elapsed.cos() * 0.5 + 0.5) * 100.0) as u16;
    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title("Texture updated each frame"))
            .gauge_style(RatatuiColor::Yellow)
            .percent(percent),
        footer,
    );
}
