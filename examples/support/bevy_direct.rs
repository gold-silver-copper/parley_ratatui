use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};
use parley_ratatui::ratatui::buffer::Buffer;
use parley_ratatui::ratatui::layout::Position;
use parley_ratatui::vello::Scene;
use parley_ratatui::vello::peniko::Color as PenikoColor;
use parley_ratatui::{GpuRenderer, TerminalRenderer, TexturePresentation};

pub struct DirectTerminalPlugin;

impl Plugin for DirectTerminalPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(ExtractResourcePlugin::<DirectTerminalFrame>::default());

        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .world_mut()
            .insert_non_send(DirectTerminalRenderState::default());
        render_app.add_systems(Render, render_terminal_frame.in_set(RenderSystems::Prepare));
    }
}

#[derive(Resource, Clone, ExtractResource)]
pub struct DirectTerminalFrame {
    image: Handle<Image>,
    width: u32,
    height: u32,
    base_color: PenikoColor,
    scene: Scene,
}

#[derive(Default)]
struct DirectTerminalRenderState {
    renderer: Option<GpuRenderer>,
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
    image.sampler = ImageSampler::linear();
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
    commands: &mut Commands,
    image: Handle<Image>,
    terminal_renderer: &mut TerminalRenderer,
    buffer: &Buffer,
    cursor: Option<Position>,
    cursor_visible: bool,
    elapsed_seconds: f32,
) {
    let (width, height) = terminal_renderer.texture_size_for_buffer(buffer);
    let base_color = terminal_renderer.theme().background.to_peniko();
    let scene = terminal_renderer
        .build_scene_with_elapsed(buffer, cursor, cursor_visible, elapsed_seconds)
        .clone();

    commands.insert_resource(DirectTerminalFrame {
        image,
        width,
        height,
        base_color,
        scene,
    });
}

fn render_terminal_frame(
    mut state: NonSendMut<DirectTerminalRenderState>,
    frame: Option<Res<DirectTerminalFrame>>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    let Some(frame) = frame else {
        return;
    };
    let Some(gpu_image) = gpu_images.get(&frame.image) else {
        return;
    };

    let size = gpu_image.texture_descriptor.size;
    if size.width != frame.width || size.height != frame.height {
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
}
