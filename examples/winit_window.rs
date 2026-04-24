use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use parley_ratatui::{ParleyBackend, PixelSize, TextureRenderer};
use ratatui::Terminal;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::Color;
use ratatui::widgets::{Block, Gauge, List, ListItem, Paragraph, Wrap};
use vello::wgpu;
use wgpu::util::TextureBlitter;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowAttributes};

fn main() -> Result<()> {
    let event_loop = EventLoop::builder().build()?;
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[derive(Default)]
struct App {
    window: Option<Arc<Window>>,
    gpu: Option<GpuState>,
    terminal: Option<Terminal<ParleyBackend>>,
    started_at: Option<Instant>,
}

struct GpuState {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    renderer: TextureRenderer,
    blitter: TextureBlitter,
    target: RenderTarget,
}

struct RenderTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: PixelSize,
}

impl RenderTarget {
    fn new(device: &wgpu::Device, size: PixelSize) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("parley_ratatui.example.texture"),
            size: wgpu::Extent3d {
                width: size.width.max(1),
                height: size.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
            size,
        }
    }

    fn resize(&mut self, device: &wgpu::Device, size: PixelSize) {
        let size = PixelSize {
            width: size.width.max(1),
            height: size.height.max(1),
        };
        if self.size != size {
            *self = Self::new(device, size);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let window = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("parley_ratatui winit texture example")
                        .with_inner_size(PhysicalSize::new(1000, 640)),
                )
                .expect("failed to create window"),
        );
        let size = window.inner_size();

        let mut terminal = Terminal::new(ParleyBackend::new(96, 30, 16.0))
            .expect("failed to create ratatui terminal");
        terminal
            .draw(|frame| render_ui(frame, 0.0))
            .expect("failed to draw initial frame");

        self.gpu = Some(
            pollster::block_on(init_gpu(
                window.clone(),
                size,
                PixelSize {
                    width: size.width,
                    height: size.height,
                },
            ))
            .expect("failed to initialize wgpu"),
        );
        self.started_at = Some(Instant::now());
        self.terminal = Some(terminal);
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    configure_surface(gpu, size);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.redraw() {
                    eprintln!("redraw failed: {error:?}");
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    fn redraw(&mut self) -> Result<()> {
        let elapsed = self
            .started_at
            .map(|started_at| started_at.elapsed().as_secs_f64())
            .unwrap_or_default();
        let terminal = self.terminal.as_mut().context("terminal not initialized")?;
        terminal.draw(|frame| render_ui(frame, elapsed))?;

        let backend = terminal.backend_mut();
        let scene = backend.build_scene();
        let gpu = self.gpu.as_mut().context("gpu not initialized")?;
        let texture_size = PixelSize {
            width: gpu.surface_config.width,
            height: gpu.surface_config.height,
        };
        gpu.target.resize(&gpu.device, texture_size);
        gpu.renderer.render_to_view(
            &gpu.device,
            &gpu.queue,
            &scene,
            &gpu.target.view,
            texture_size,
            backend.clear_color(),
        )?;

        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                gpu.surface.configure(&gpu.device, &gpu.surface_config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error) => return Err(error).context("failed to acquire surface texture"),
        };

        let surface_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("parley_ratatui.example.blit"),
            });
        gpu.blitter
            .copy(&gpu.device, &mut encoder, &gpu.target.view, &surface_view);
        gpu.queue.submit(Some(encoder.finish()));
        frame.present();
        let _ = gpu.device.poll(wgpu::PollType::Poll);
        Ok(())
    }
}

async fn init_gpu(
    window: Arc<Window>,
    window_size: PhysicalSize<u32>,
    texture_size: PixelSize,
) -> Result<GpuState> {
    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window)?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await?;
    let maybe_features = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("parley_ratatui.example.device"),
            required_features: adapter.features() & maybe_features,
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::default(),
            experimental_features: Default::default(),
        })
        .await?;

    let surface_config = surface
        .get_default_config(
            &adapter,
            window_size.width.max(1),
            window_size.height.max(1),
        )
        .context("surface is not supported by the selected adapter")?;
    surface.configure(&device, &surface_config);

    let renderer = TextureRenderer::new(&device)?;
    let blitter = TextureBlitter::new(&device, surface_config.format);
    let target = RenderTarget::new(&device, texture_size);

    Ok(GpuState {
        surface,
        device,
        queue,
        surface_config,
        renderer,
        blitter,
        target,
    })
}

fn configure_surface(gpu: &mut GpuState, size: PhysicalSize<u32>) {
    if size.width == 0 || size.height == 0 {
        return;
    }

    gpu.surface_config.width = size.width;
    gpu.surface_config.height = size.height;
    gpu.surface.configure(&gpu.device, &gpu.surface_config);
}

fn render_ui(frame: &mut ratatui::Frame<'_>, elapsed: f64) {
    let area = frame.area();
    let [header, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .areas(area);

    let title = Paragraph::new(format!(
        "parley_ratatui + winit + vello/wgpu  {:.1}s",
        elapsed
    ))
    .block(Block::bordered().title("Winit"))
    .alignment(Alignment::Center)
    .style(Color::Cyan);
    frame.render_widget(title, header);

    let [left, right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .areas(body);

    let items = [
        ListItem::new("Ratatui draws into ParleyBackend"),
        ListItem::new("Parley shapes glyphs with font fallback"),
        ListItem::new("Vello renders the scene to an offscreen texture"),
        ListItem::new("wgpu blits that texture to the window surface"),
    ];
    frame.render_widget(
        List::new(items)
            .block(Block::bordered().title("Pipeline"))
            .highlight_style(Color::Yellow),
        left,
    );

    let percent = ((elapsed.sin() * 0.5 + 0.5) * 100.0) as u16;
    let copy = Paragraph::new(
        "This is intentionally the same ownership model a game engine would use: the \
         window owns the device and target, while the backend only owns terminal state.",
    )
    .block(Block::bordered().title("Texture target"))
    .wrap(Wrap { trim: true });
    frame.render_widget(copy, right);
    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title("Animated gauge"))
            .gauge_style(Color::Green)
            .percent(percent),
        footer,
    );
}
