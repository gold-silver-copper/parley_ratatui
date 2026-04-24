use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::style::{Color, Stylize};
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph};
use parley_ratatui::{FontOptions, ParleyBackend, TerminalRenderer, Theme};

#[derive(Resource)]
struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    handle: Handle<Image>,
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_systems(Startup, setup)
        .add_systems(Update, update_terminal_texture)
        .run();
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.spawn(Camera2d);

    let terminal = Terminal::new(ParleyBackend::new(52, 16)).expect("terminal");
    let renderer = TerminalRenderer::new(FontOptions::default(), Theme::default());
    let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());

    let image = Image::new_fill(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[17, 24, 39, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    let handle = images.add(image);

    commands.spawn(Sprite {
        image: handle.clone(),
        custom_size: Some(Vec2::new(width as f32, height as f32)),
        ..default()
    });

    commands.insert_resource(TerminalTexture {
        terminal,
        renderer,
        handle,
    });
}

fn update_terminal_texture(
    mut terminal_texture: ResMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
    time: Res<Time>,
) {
    terminal_texture
        .terminal
        .draw(|frame| {
            let area = frame.area();
            let title = format!(
                "Parley Ratatui -> Bevy Texture  {:.2}s",
                time.elapsed_secs()
            );
            let paragraph = Paragraph::new(vec![
                "This example renders Ratatui into an Image asset.".into(),
                "Use TerminalRenderer::render_to_texture with Bevy's RenderDevice for GPU output."
                    .into(),
                "The same backend buffer drives both paths."
                    .fg(Color::LightCyan)
                    .into(),
            ])
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

    // Bevy owns the swapchain device, so the example updates an Image asset on the main world.
    // Integrations that run in the render world can instead create `TextureTarget` and call
    // `TerminalRenderer::render_to_texture` with Bevy's RenderDevice and RenderQueue.
    let buffer = terminal_texture.terminal.backend().buffer().clone();
    let cursor_position = terminal_texture.terminal.backend().cursor_position();
    let cursor_visible = terminal_texture.terminal.backend().cursor_visible();
    let _ = terminal_texture
        .renderer
        .build_scene(&buffer, Some(cursor_position), cursor_visible);
}
