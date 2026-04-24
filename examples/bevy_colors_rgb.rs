use std::time::{Duration, Instant};

use bevy::app::AppExit;
use bevy::asset::RenderAssetUsages;
use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat as BevyTextureFormat,
};
use palette::{Okhsv, Srgb, convert::FromColorUnclamped};
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::buffer::Buffer;
use parley_ratatui::ratatui::layout::{Constraint, Layout, Position, Rect};
use parley_ratatui::ratatui::style::Color;
use parley_ratatui::ratatui::text::Text;
use parley_ratatui::ratatui::widgets::Widget;
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureTarget, Theme,
};

struct TerminalTexture {
    terminal: Terminal<ParleyBackend>,
    terminal_app: ColorsRgbApp,
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

#[derive(Debug, Default)]
struct ColorsRgbApp {
    fps_widget: FpsWidget,
    colors_widget: ColorsWidget,
}

#[derive(Debug)]
struct FpsWidget {
    frame_count: usize,
    last_instant: Instant,
    fps: Option<f32>,
}

#[derive(Debug, Default)]
struct ColorsWidget {
    colors: Vec<Vec<Color>>,
    frame_count: usize,
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
            Some("parley_ratatui.bevy_colors_rgb"),
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
            Some("parley_ratatui.bevy_colors_rgb"),
        );
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .add_systems(Startup, setup)
        .add_systems(Update, (update_terminal_texture, exit_on_key))
        .run();
}

fn setup(world: &mut World) {
    let terminal = Terminal::new(ParleyBackend::new(120, 42)).expect("terminal");
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
        terminal_app: ColorsRgbApp::default(),
        renderer,
        gpu,
        handle,
    });
}

fn update_terminal_texture(
    mut terminal_texture: NonSendMut<TerminalTexture>,
    mut images: ResMut<Assets<Image>>,
) {
    let terminal_texture = &mut *terminal_texture;
    let terminal_app = &mut terminal_texture.terminal_app;
    terminal_texture
        .terminal
        .draw(|frame| frame.render_widget(terminal_app, frame.area()))
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

fn exit_on_key(keys: Res<ButtonInput<KeyCode>>, mut app_exit_writer: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::KeyQ) || keys.just_pressed(KeyCode::Escape) {
        app_exit_writer.write(AppExit::Success);
    }
}

impl Widget for &mut ColorsRgbApp {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};

        let [top, colors] = Layout::vertical([Length(1), Min(0)]).areas(area);
        let [title, fps] = Layout::horizontal([Min(0), Length(8)]).areas(top);
        Text::from("colors_rgb in Bevy texture. Press q or Esc to quit")
            .centered()
            .render(title, buf);
        self.fps_widget.render(fps, buf);
        self.colors_widget.render(colors, buf);
    }
}

impl Default for FpsWidget {
    fn default() -> Self {
        Self {
            frame_count: 0,
            last_instant: Instant::now(),
            fps: None,
        }
    }
}

impl Widget for &mut FpsWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.calculate_fps();
        if let Some(fps) = self.fps {
            Text::from(format!("{fps:.1} fps")).render(area, buf);
        }
    }
}

impl FpsWidget {
    fn calculate_fps(&mut self) {
        self.frame_count += 1;
        let elapsed = self.last_instant.elapsed();
        if elapsed > Duration::from_secs(1) && self.frame_count > 2 {
            self.fps = Some(self.frame_count as f32 / elapsed.as_secs_f32());
            self.frame_count = 0;
            self.last_instant = Instant::now();
        }
    }
}

impl Widget for &mut ColorsWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.setup_colors(area);
        let colors = &self.colors;
        for (xi, x) in (area.left()..area.right()).enumerate() {
            let xi = (xi + self.frame_count) % (area.width as usize);
            for (yi, y) in (area.top()..area.bottom()).enumerate() {
                let fg = colors[yi * 2][xi];
                let bg = colors[yi * 2 + 1][xi];
                buf[Position::new(x, y)].set_char('▀').set_fg(fg).set_bg(bg);
            }
        }
        self.frame_count += 1;
    }
}

impl ColorsWidget {
    fn setup_colors(&mut self, size: Rect) {
        let Rect { width, height, .. } = size;
        let height = height as usize * 2;
        let width = width as usize;
        if self.colors.len() == height && self.colors.first().is_some_and(|row| row.len() == width)
        {
            return;
        }

        self.colors = Vec::with_capacity(height);
        for y in 0..height {
            let mut row = Vec::with_capacity(width);
            for x in 0..width {
                let hue = x as f32 * 360.0 / width as f32;
                let value = (height - y) as f32 / height as f32;
                let saturation = Okhsv::max_saturation();
                let color = Okhsv::new(hue, saturation, value);
                let color = Srgb::<f32>::from_color_unclamped(color);
                let color: Srgb<u8> = color.into_format();
                row.push(Color::Rgb(color.red, color.green, color.blue));
            }
            self.colors.push(row);
        }
    }
}
