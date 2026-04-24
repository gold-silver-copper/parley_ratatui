use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Position;
use ratatui::style::{Modifier, Style};
use std::sync::mpsc;
use unicode_width::UnicodeWidthStr;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Brush, Fill};
use vello::{AaConfig, Glyph, RenderParams, Renderer, RendererOptions, Scene, wgpu};

use crate::color::Rgba;
use crate::color::Theme;
use crate::text::{FontOptions, TextMetrics, TextStyle, TextSystem};

#[derive(Debug)]
pub enum RenderError {
    CreateRenderer(vello::Error),
    Render(vello::Error),
    ReadbackFormat(wgpu::TextureFormat),
    CreateReadback(wgpu::BufferAsyncError),
    Poll(wgpu::PollError),
    ReadbackCanceled,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateRenderer(err) => write!(f, "failed to create vello renderer: {err}"),
            Self::Render(err) => write!(f, "failed to render vello scene: {err}"),
            Self::ReadbackFormat(format) => {
                write!(f, "unsupported texture readback format: {format:?}")
            }
            Self::CreateReadback(err) => write!(f, "failed to map texture readback buffer: {err}"),
            Self::Poll(err) => write!(f, "failed to poll wgpu device for readback: {err}"),
            Self::ReadbackCanceled => write!(f, "texture readback callback was canceled"),
        }
    }
}

impl std::error::Error for RenderError {}

pub struct TextureTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
}

impl TextureTarget {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        label: impl Into<Option<&'static str>>,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: label.into(),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            width: width.max(1),
            height: height.max(1),
            format,
        }
    }
}

pub struct TerminalRenderer {
    text: TextSystem,
    theme: Theme,
    scene: Scene,
}

#[derive(Clone, Copy, Debug, Default)]
struct BlinkState {
    hide_slow: bool,
    hide_rapid: bool,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedCellStyle {
    fg: Rgba,
    bg: Rgba,
    display_width: u16,
}

pub struct GpuRenderer {
    renderer: Renderer,
}

impl GpuRenderer {
    pub fn new(device: &wgpu::Device) -> Result<Self, RenderError> {
        let renderer = Renderer::new(device, RendererOptions::default())
            .map_err(RenderError::CreateRenderer)?;
        Ok(Self { renderer })
    }

    pub fn render_to_texture(
        &mut self,
        terminal: &mut TerminalRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) -> Result<(), RenderError> {
        let base_color = terminal.theme.background.to_peniko();
        let scene = terminal.build_scene(buffer, cursor, cursor_visible);
        self.renderer
            .render_to_texture(
                device,
                queue,
                scene,
                &target.view,
                &RenderParams {
                    base_color,
                    width: target.width,
                    height: target.height,
                    antialiasing_method: AaConfig::Msaa8,
                },
            )
            .map_err(RenderError::Render)
    }

    pub fn render_to_texture_with_elapsed(
        &mut self,
        terminal: &mut TerminalRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
        elapsed_seconds: f32,
    ) -> Result<(), RenderError> {
        let base_color = terminal.theme.background.to_peniko();
        let scene =
            terminal.build_scene_with_elapsed(buffer, cursor, cursor_visible, elapsed_seconds);
        self.renderer
            .render_to_texture(
                device,
                queue,
                scene,
                &target.view,
                &RenderParams {
                    base_color,
                    width: target.width,
                    height: target.height,
                    antialiasing_method: AaConfig::Msaa8,
                },
            )
            .map_err(RenderError::Render)
    }

    pub fn render_to_rgba8(
        &mut self,
        terminal: &mut TerminalRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) -> Result<Vec<u8>, RenderError> {
        self.render_to_texture(
            terminal,
            device,
            queue,
            target,
            buffer,
            cursor,
            cursor_visible,
        )?;
        read_texture_to_rgba8(device, queue, target)
    }

    pub fn render_to_rgba8_with_elapsed(
        &mut self,
        terminal: &mut TerminalRenderer,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
        elapsed_seconds: f32,
    ) -> Result<Vec<u8>, RenderError> {
        self.render_to_texture_with_elapsed(
            terminal,
            device,
            queue,
            target,
            buffer,
            cursor,
            cursor_visible,
            elapsed_seconds,
        )?;
        read_texture_to_rgba8(device, queue, target)
    }
}

impl TerminalRenderer {
    pub fn new(font: FontOptions, theme: Theme) -> Self {
        Self {
            text: TextSystem::new(font),
            theme,
            scene: Scene::new(),
        }
    }

    pub fn metrics(&self) -> TextMetrics {
        self.text.metrics()
    }

    pub fn texture_size_for_buffer(&self, buffer: &Buffer) -> (u32, u32) {
        let metrics = self.metrics();
        (
            (f32::from(buffer.area.width) * metrics.cell_width)
                .ceil()
                .max(1.0) as u32,
            (f32::from(buffer.area.height) * metrics.cell_height)
                .ceil()
                .max(1.0) as u32,
        )
    }

    pub fn build_scene(
        &mut self,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) -> &Scene {
        self.build_scene_inner(buffer, cursor, cursor_visible, BlinkState::default())
    }

    pub fn build_scene_with_elapsed(
        &mut self,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
        elapsed_seconds: f32,
    ) -> &Scene {
        self.build_scene_inner(
            buffer,
            cursor,
            cursor_visible,
            BlinkState::from_elapsed(elapsed_seconds),
        )
    }

    fn build_scene_inner(
        &mut self,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
        blink: BlinkState,
    ) -> &Scene {
        self.scene.reset();

        let metrics = self.text.metrics();
        let width = f32::from(buffer.area.width) * metrics.cell_width;
        let height = f32::from(buffer.area.height) * metrics.cell_height;
        fill_rect(
            &mut self.scene,
            0.0,
            0.0,
            width,
            height,
            self.theme.background.to_peniko(),
        );

        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let style = self.resolve_buffer_cell_style(buffer, x, y, blink);
                self.paint_cell_background(x, y, style);
            }
        }

        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                let cell = &buffer[(x, y)];
                let style = self.resolve_buffer_cell_style(buffer, x, y, blink);
                self.paint_cell_text_and_decorations(x, y, cell, style);
            }
        }

        if cursor_visible
            && let Some(position) = cursor
            && position.x < buffer.area.width
            && position.y < buffer.area.height
        {
            fill_rect(
                &mut self.scene,
                f32::from(position.x) * metrics.cell_width,
                f32::from(position.y) * metrics.cell_height,
                metrics.cell_width,
                metrics.cell_height,
                self.theme.cursor.to_peniko(),
            );
        }

        &self.scene
    }

    pub fn render_to_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) -> Result<(), RenderError> {
        GpuRenderer::new(device)?.render_to_texture(
            self,
            device,
            queue,
            target,
            buffer,
            cursor,
            cursor_visible,
        )
    }

    pub fn render_to_rgba8(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &TextureTarget,
        buffer: &Buffer,
        cursor: Option<Position>,
        cursor_visible: bool,
    ) -> Result<Vec<u8>, RenderError> {
        GpuRenderer::new(device)?.render_to_rgba8(
            self,
            device,
            queue,
            target,
            buffer,
            cursor,
            cursor_visible,
        )
    }

    fn paint_cell_background(&mut self, x: u16, y: u16, style: ResolvedCellStyle) {
        let metrics = self.text.metrics();
        let x_px = f32::from(x) * metrics.cell_width;
        let y_px = f32::from(y) * metrics.cell_height;
        fill_rect(
            &mut self.scene,
            x_px,
            y_px,
            metrics.cell_width * f32::from(style.display_width),
            metrics.cell_height,
            style.bg.to_peniko(),
        );
    }

    fn paint_cell_text_and_decorations(
        &mut self,
        x: u16,
        y: u16,
        cell: &Cell,
        resolved: ResolvedCellStyle,
    ) {
        let metrics = self.text.metrics();
        let style = cell.style();
        let x_px = f32::from(x) * metrics.cell_width;
        let y_px = f32::from(y) * metrics.cell_height;
        let cell_width = metrics.cell_width * f32::from(resolved.display_width);

        let symbol = cell.symbol();
        if should_shape_text(symbol) && !style.add_modifier.contains(Modifier::HIDDEN) {
            let layout = self.text.shape(
                symbol,
                TextStyle {
                    bold: style.add_modifier.contains(Modifier::BOLD),
                    italic: style.add_modifier.contains(Modifier::ITALIC),
                },
            );
            paint_layout(
                &mut self.scene,
                &layout,
                x_px,
                y_px,
                resolved.fg.to_peniko(),
            );
        }

        if style.add_modifier.contains(Modifier::UNDERLINED) {
            let (line_y, thickness) = decoration_geometry(
                metrics,
                y_px,
                metrics.underline_position,
                metrics.underline_thickness,
            );
            fill_rect(
                &mut self.scene,
                x_px,
                line_y,
                cell_width,
                thickness,
                resolved.fg.to_peniko(),
            );
        }
        if style.add_modifier.contains(Modifier::CROSSED_OUT) {
            let (line_y, thickness) = decoration_geometry(
                metrics,
                y_px,
                metrics.strikeout_position,
                metrics.strikeout_thickness,
            );
            fill_rect(
                &mut self.scene,
                x_px,
                line_y,
                cell_width,
                thickness,
                resolved.fg.to_peniko(),
            );
        }
    }

    fn resolve_buffer_cell_style(
        &self,
        buffer: &Buffer,
        x: u16,
        y: u16,
        blink: BlinkState,
    ) -> ResolvedCellStyle {
        let cell = &buffer[(x, y)];
        let mut style = self.resolve_cell_style(cell.style(), cell.symbol(), blink);

        if cell.symbol() == " "
            && cell.style().bg.is_none()
            && let Some(owner_style) = self.previous_wide_cell_style(buffer, x, y, blink)
        {
            style.fg = owner_style.fg;
            style.bg = owner_style.bg;
            style.display_width = 1;
        }

        style
    }

    fn previous_wide_cell_style(
        &self,
        buffer: &Buffer,
        x: u16,
        y: u16,
        blink: BlinkState,
    ) -> Option<ResolvedCellStyle> {
        if x == 0 {
            return None;
        }

        let previous = &buffer[(x - 1, y)];
        (display_width(previous.symbol()) > 1)
            .then(|| self.resolve_cell_style(previous.style(), previous.symbol(), blink))
    }

    fn resolve_cell_style(
        &self,
        style: Style,
        symbol: &str,
        blink: BlinkState,
    ) -> ResolvedCellStyle {
        let mut fg = self.theme.foreground(style);
        let mut bg = self.theme.background(style);
        if style.add_modifier.contains(Modifier::REVERSED) {
            std::mem::swap(&mut fg, &mut bg);
        }

        if style.add_modifier.contains(Modifier::HIDDEN)
            || (style.add_modifier.contains(Modifier::SLOW_BLINK) && blink.hide_slow)
            || (style.add_modifier.contains(Modifier::RAPID_BLINK) && blink.hide_rapid)
        {
            fg = bg;
        }

        ResolvedCellStyle {
            fg,
            bg,
            display_width: display_width(symbol).clamp(1, 2),
        }
    }
}

impl BlinkState {
    fn from_elapsed(elapsed_seconds: f32) -> Self {
        Self {
            hide_slow: blink_hidden(elapsed_seconds, 0.5),
            hide_rapid: blink_hidden(elapsed_seconds, 0.25),
        }
    }
}

fn paint_layout(
    scene: &mut Scene,
    layout: &parley::Layout<()>,
    x: f32,
    y: f32,
    color: vello::peniko::Color,
) {
    let brush = Brush::Solid(color);
    let transform = Affine::translate((f64::from(x), f64::from(y)));
    for line in layout.lines() {
        for item in line.items() {
            let parley::layout::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };

            let run = glyph_run.run();
            let font = run.font();
            let font_size = run.font_size();
            let mut x = glyph_run.offset();
            let y = glyph_run.baseline();
            scene
                .draw_glyphs(font)
                .brush(&brush)
                .hint(false)
                .transform(transform)
                .font_size(font_size)
                .normalized_coords(run.normalized_coords())
                .draw(
                    Fill::NonZero,
                    glyph_run.glyphs().map(|glyph| {
                        let gx = x + glyph.x;
                        let gy = y - glyph.y;
                        x += glyph.advance;
                        Glyph {
                            id: glyph.id as u32,
                            x: gx,
                            y: gy,
                        }
                    }),
                );
        }
    }
}

fn fill_rect(
    scene: &mut Scene,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: vello::peniko::Color,
) {
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color,
        None,
        &Rect::new(
            f64::from(x),
            f64::from(y),
            f64::from(x + width),
            f64::from(y + height),
        ),
    );
}

fn should_shape_text(symbol: &str) -> bool {
    symbol.chars().any(|character| !character.is_whitespace())
}

fn display_width(symbol: &str) -> u16 {
    UnicodeWidthStr::width(symbol).max(1).min(u16::MAX as usize) as u16
}

fn decoration_geometry(
    metrics: TextMetrics,
    y_px: f32,
    position: f32,
    thickness: f32,
) -> (f32, f32) {
    let thickness = thickness.round().max(1.0);
    let y = ((y_px + metrics.baseline - position) - thickness / 2.0)
        .round()
        .min(y_px + metrics.cell_height - thickness);
    (y, thickness)
}

fn blink_hidden(elapsed_seconds: f32, half_period_seconds: f32) -> bool {
    ((elapsed_seconds / half_period_seconds).floor() as u64) % 2 == 1
}

fn read_texture_to_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &TextureTarget,
) -> Result<Vec<u8>, RenderError> {
    if !matches!(
        target.format,
        wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb
    ) {
        return Err(RenderError::ReadbackFormat(target.format));
    }

    let bytes_per_pixel = 4;
    let unpadded_bytes_per_row = target.width * bytes_per_pixel;
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer_size = u64::from(padded_bytes_per_row) * u64::from(target.height);

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parley_ratatui.readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("parley_ratatui.readback_encoder"),
    });
    encoder.copy_texture_to_buffer(
        target.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(target.height),
            },
        },
        wgpu::Extent3d {
            width: target.width,
            height: target.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    let (sender, receiver) = mpsc::sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(RenderError::Poll)?;
    receiver
        .recv()
        .map_err(|_| RenderError::ReadbackCanceled)?
        .map_err(RenderError::CreateReadback)?;

    let mapped = slice.get_mapped_range();
    let mut rgba = vec![0; (target.width * target.height * bytes_per_pixel) as usize];
    for y in 0..target.height as usize {
        let src_start = y * padded_bytes_per_row as usize;
        let src_end = src_start + unpadded_bytes_per_row as usize;
        let dst_start = y * unpadded_bytes_per_row as usize;
        let dst_end = dst_start + unpadded_bytes_per_row as usize;
        rgba[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
    }
    drop(mapped);
    readback.unmap();

    Ok(rgba)
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
