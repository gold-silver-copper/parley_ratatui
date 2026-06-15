use std::sync::{Arc, Mutex};

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use parley_ratatui::ratatui::buffer::Buffer;
use parley_ratatui::ratatui::layout::Position;
use parley_ratatui::vello::Scene;
use parley_ratatui::vello::peniko::Color as PenikoColor;
use parley_ratatui::{GpuRenderer, TerminalRenderer, TexturePresentation};

pub struct DirectTerminalPlugin;

impl Plugin for DirectTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DirectTerminalSceneExchange>();
        let exchange = app
            .world()
            .resource::<DirectTerminalSceneExchange>()
            .clone();

        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .world_mut()
            .insert_non_send(DirectTerminalRenderState::default());
        render_app.world_mut().insert_resource(exchange);
        render_app.init_resource::<ExtractedDirectTerminalFrame>();
        render_app.add_systems(ExtractSchedule, extract_terminal_frame);
        render_app.add_systems(Render, render_terminal_frame.in_set(RenderSystems::Prepare));
    }
}

pub struct DirectTerminalFrame {
    image: Handle<Image>,
    width: u32,
    height: u32,
    base_color: PenikoColor,
    scene: Scene,
}

#[derive(Resource, Clone, Default)]
pub struct DirectTerminalSceneExchange {
    inner: Arc<DirectTerminalSceneExchangeInner>,
}

#[derive(Default)]
struct DirectTerminalSceneExchangeInner {
    pending: Mutex<Option<DirectTerminalFrame>>,
    recycled: Mutex<Option<Scene>>,
}

#[derive(Default)]
struct DirectTerminalRenderState {
    renderer: Option<GpuRenderer>,
}

#[derive(Resource, Default)]
struct ExtractedDirectTerminalFrame(Option<DirectTerminalFrame>);

impl DirectTerminalSceneExchange {
    fn take_recycled_scene(&self) -> Scene {
        self.inner
            .recycled
            .lock()
            .expect("direct terminal recycled scene lock")
            .take()
            .unwrap_or_default()
    }

    fn recycle_scene(&self, mut scene: Scene) {
        scene.reset();

        let mut recycled = self
            .inner
            .recycled
            .lock()
            .expect("direct terminal recycled scene lock");
        if recycled.is_none() {
            *recycled = Some(scene);
        }
    }

    fn publish_frame(&self, frame: DirectTerminalFrame) {
        let previous = self
            .inner
            .pending
            .lock()
            .expect("direct terminal pending frame lock")
            .replace(frame);

        if let Some(previous) = previous {
            self.recycle_scene(previous.scene);
        }
    }

    fn take_pending_frame(&self) -> Option<DirectTerminalFrame> {
        self.inner
            .pending
            .lock()
            .expect("direct terminal pending frame lock")
            .take()
    }
}

pub fn new_terminal_image(width: u32, height: u32, label: &'static str) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.texture_descriptor.label = Some(label);
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::STORAGE_BINDING;
    // The texture is authored at physical resolution and presented 1:1, so use
    // point sampling: a terminal wants crisp pixel-aligned cells, and nearest
    // avoids any antialiased bleed at cell edges if the present is ever slightly
    // off the pixel grid.
    image.sampler = ImageSampler::nearest();
    image
}

pub fn resize_terminal_image(image: &mut Image, width: u32, height: u32) {
    if image.texture_descriptor.size.width == width
        && image.texture_descriptor.size.height == height
    {
        return;
    }

    image.resize(Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    });
}

#[allow(dead_code)]
pub fn texture_logical_size(width: u32, height: u32, render_scale: f32) -> Vec2 {
    let [width, height] = TexturePresentation::new([width, height], render_scale).logical_size();
    Vec2::new(width, height)
}

pub fn update_direct_terminal_frame(
    exchange: &DirectTerminalSceneExchange,
    image: Handle<Image>,
    terminal_renderer: &mut TerminalRenderer,
    buffer: &Buffer,
    cursor: Option<Position>,
    cursor_visible: bool,
    elapsed_seconds: f32,
) {
    let build_scene = exchange.take_recycled_scene();
    let spare_scene = terminal_renderer.replace_scene(build_scene);
    let (width, height) = terminal_renderer.texture_size_for_buffer(buffer);
    let base_color = terminal_renderer.theme().background.to_peniko();
    terminal_renderer.build_scene_with_elapsed(buffer, cursor, cursor_visible, elapsed_seconds);
    let scene = terminal_renderer.replace_scene(spare_scene);

    exchange.publish_frame(DirectTerminalFrame {
        image,
        width,
        height,
        base_color,
        scene,
    });
}

fn extract_terminal_frame(
    mut frame: ResMut<ExtractedDirectTerminalFrame>,
    exchange: Extract<Res<DirectTerminalSceneExchange>>,
) {
    if let Some(next_frame) = exchange.take_pending_frame() {
        if let Some(previous_frame) = frame.0.replace(next_frame) {
            exchange.recycle_scene(previous_frame.scene);
        }
    }
}

fn render_terminal_frame(
    mut state: NonSendMut<DirectTerminalRenderState>,
    exchange: Res<DirectTerminalSceneExchange>,
    mut frame: ResMut<ExtractedDirectTerminalFrame>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    let Some(frame) = frame.0.take() else {
        return;
    };
    let Some(gpu_image) = gpu_images.get(&frame.image) else {
        exchange.recycle_scene(frame.scene);
        return;
    };

    let size = gpu_image.texture_descriptor.size;
    if size.width != frame.width || size.height != frame.height {
        exchange.recycle_scene(frame.scene);
        return;
    }

    let device = render_device.wgpu_device();
    let renderer = state
        .renderer
        .get_or_insert_with(|| GpuRenderer::new(device).expect("vello renderer"));

    renderer
        .render_scene_to_texture_view(
            device,
            &render_queue,
            &gpu_image.texture_view,
            frame.width,
            frame.height,
            frame.base_color,
            &frame.scene,
        )
        .expect("render terminal scene into Bevy texture");

    exchange.recycle_scene(frame.scene);
}
