use std::sync::{Arc, Mutex};

use bevy::asset::{RenderAssetUsages, load_internal_asset, uuid_handle};
use bevy::image::ImageSampler;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology, VertexBufferLayout};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    AsBindGroup, BlendState, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat, TextureUsages, VertexFormat, VertexStepMode,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use bevy::shader::{Shader, ShaderRef};
use bevy::sprite_render::{Material2d, Material2dKey, Material2dPlugin};
use parley_ratatui::ratatui::buffer::Buffer;
use parley_ratatui::ratatui::layout::Position;
use parley_ratatui::vello::Scene;
use parley_ratatui::vello::peniko::Color as PenikoColor;
use parley_ratatui::{GpuRenderer, TerminalRenderer, TexturePresentation};

/// Handle for the embedded 1:1 terminal-present shader.
const TERMINAL_PRESENT_SHADER: Handle<Shader> =
    uuid_handle!("b2c4e6a8-1357-4f9b-8d2e-3a5c7e9b1d4f");

pub struct DirectTerminalPlugin;

impl Plugin for DirectTerminalPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            TERMINAL_PRESENT_SHADER,
            "terminal_present.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(Material2dPlugin::<TerminalPresentMaterial>::default());
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

/// Material that presents the terminal texture 1:1 with physical pixels.
///
/// A fullscreen quad whose fragment shader fetches each texel by physical pixel
/// coordinate (`textureLoad`) — an identity sample with no resampling, the crisp
/// presentation pattern from linebender/bevy_vello. Sampled via `textureLoad`, so
/// no sampler binding is needed.
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub struct TerminalPresentMaterial {
    /// The terminal texture (a Vello-rendered `Rgba8Unorm` storage texture).
    #[texture(0)]
    pub texture: Handle<Image>,
}

impl Material2d for TerminalPresentMaterial {
    fn vertex_shader() -> ShaderRef {
        TERMINAL_PRESENT_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        TERMINAL_PRESENT_SHADER.into()
    }

    fn specialize(
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // Alpha-blend so the area outside the terminal shows the camera clear.
        if let Some(fragment) = descriptor.fragment.as_mut()
            && let Some(Some(target)) = fragment.targets.first_mut()
        {
            target.blend = Some(BlendState::ALPHA_BLENDING);
        }
        descriptor.vertex.buffers = vec![VertexBufferLayout::from_vertex_formats(
            VertexStepMode::Vertex,
            [VertexFormat::Float32x3],
        )];
        Ok(())
    }
}

/// A clip-space triangle covering the whole viewport (positions only).
#[allow(dead_code)]
pub fn present_quad() -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[-1.0, -1.0, 0.0], [3.0, -1.0, 0.0], [-1.0, 3.0, 0.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
    mesh
}

/// The presenting window's physical/logical pixel ratio, so the terminal texture
/// is rendered at exactly framebuffer resolution and presents 1:1.
///
/// Derived from the real physical size rather than the reported scale factor,
/// which can leak a higher-DPI monitor's scale on a mixed-DPI setup and over-size
/// the texture (forcing a resample).
#[allow(dead_code)]
pub fn render_scale_for_window(window: &Window) -> f32 {
    let logical = window.resolution.size().max(Vec2::ONE);
    let physical = window.resolution.physical_size().as_vec2();
    (physical.x / logical.x)
        .min(physical.y / logical.y)
        .max(1.0)
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
