use eframe::egui;
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::layout::{Constraint, Layout, Rect};
use parley_ratatui::ratatui::style::{Color, Modifier, Style};
use parley_ratatui::ratatui::text::{Line, Span};
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph, Widget};
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    AsyncTextureReadback, FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureTarget,
    Theme,
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([980.0, 740.0]),
        ..Default::default()
    };

    eframe::run_native(
        "parley_ratatui egui resizable",
        options,
        Box::new(|cc| Ok(Box::new(ResizableTerminalApp::new(cc)))),
    )
}

struct ResizableTerminalApp {
    terminal: Terminal<ParleyBackend>,
    renderer: TerminalRenderer,
    gpu: OffscreenGpu,
    rgba: Vec<u8>,
    texture: Option<egui::TextureHandle>,
    frame_count: u64,
}

struct OffscreenGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: GpuRenderer,
    target: TextureTarget,
    readback: AsyncTextureReadback,
}

impl ResizableTerminalApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let terminal = Terminal::new(ParleyBackend::new(80, 24)).expect("terminal");
        let renderer = TerminalRenderer::new(example_font_options(), Theme::default());
        let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
        let gpu = pollster::block_on(OffscreenGpu::new(width, height));

        Self {
            terminal,
            renderer,
            gpu,
            rgba: Vec::new(),
            texture: None,
            frame_count: 0,
        }
    }

    fn show_terminal_widget(&mut self, ui: &mut egui::Ui) {
        egui::Resize::default()
            .id_salt("terminal_resize")
            .default_size(egui::vec2(840.0, 520.0))
            .min_size(egui::vec2(260.0, 180.0))
            .show(ui, |ui| {
                let size_points = ui.available_size_before_wrap();
                let image_size_points = self.render_for_size(ui.ctx(), size_points);

                let (rect, _) = ui.allocate_exact_size(size_points, egui::Sense::hover());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(17, 24, 39));

                if let Some(texture) = &self.texture {
                    let image_origin =
                        snap_to_physical_pixels(rect.min, ui.ctx().pixels_per_point());
                    let image_rect = egui::Rect::from_min_size(image_origin, image_size_points);
                    painter.image(
                        texture.id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            });
    }

    fn render_for_size(&mut self, ctx: &egui::Context, available_points: egui::Vec2) -> egui::Vec2 {
        let pixels_per_point = ctx.pixels_per_point();
        let (columns, rows) = self.cells_for_size(available_points, pixels_per_point);

        let current_area = self.terminal.backend().buffer().area;
        if current_area.width != columns || current_area.height != rows {
            self.terminal.backend_mut().resize(columns, rows);
        }

        let (width, height) = self
            .renderer
            .texture_size_for_buffer(self.terminal.backend().buffer());
        let resized = self.gpu.resize(width, height);
        if resized {
            self.gpu.readback = AsyncTextureReadback::new();
            self.rgba.clear();
            self.texture = None;
        }

        if self
            .gpu
            .readback
            .try_read_rgba8_into(&self.gpu.device, &mut self.rgba)
            .expect("read terminal texture")
        {
            self.upload_texture(ctx);
        }

        self.draw_terminal(columns, rows);
        self.render_and_submit();

        egui::vec2(
            width as f32 / pixels_per_point,
            height as f32 / pixels_per_point,
        )
    }

    fn cells_for_size(&self, available_points: egui::Vec2, pixels_per_point: f32) -> (u16, u16) {
        let metrics = self.renderer.metrics();
        let available_width = available_points.x.max(1.0) * pixels_per_point;
        let available_height = available_points.y.max(1.0) * pixels_per_point;

        let columns = (available_width / metrics.cell_width)
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16;
        let rows = (available_height / metrics.cell_height)
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16;
        (columns, rows)
    }

    fn upload_texture(&mut self, ctx: &egui::Context) {
        let width = self.gpu.target.width as usize;
        let height = self.gpu.target.height as usize;
        if self.rgba.len() != width * height * 4 {
            return;
        }

        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &self.rgba);
        if let Some(texture) = &mut self.texture {
            texture.set(image, egui::TextureOptions::NEAREST);
        } else {
            self.texture = Some(ctx.load_texture(
                "parley_ratatui_terminal",
                image,
                egui::TextureOptions::NEAREST,
            ));
        }
    }

    fn draw_terminal(&mut self, columns: u16, rows: u16) {
        let frame_count = self.frame_count;
        self.terminal
            .draw(|frame| {
                let area = frame.area();
                TerminalDemo {
                    columns,
                    rows,
                    frame_count,
                }
                .render(area, frame.buffer_mut());
            })
            .expect("draw terminal");
        self.frame_count = self.frame_count.wrapping_add(1);
    }

    fn render_and_submit(&mut self) {
        let cursor_position = self.terminal.backend().cursor_position();
        let cursor_visible = self.terminal.backend().cursor_visible();
        let buffer = self.terminal.backend().buffer();
        self.gpu
            .renderer
            .render_to_texture(
                &mut self.renderer,
                &self.gpu.device,
                &self.gpu.queue,
                &self.gpu.target,
                buffer,
                Some(cursor_position),
                cursor_visible,
            )
            .expect("render terminal texture");
        self.gpu
            .readback
            .submit(&self.gpu.device, &self.gpu.queue, &self.gpu.target)
            .expect("submit terminal texture readback");
    }
}

impl eframe::App for ResizableTerminalApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.label("Drag the bottom-right handle. The terminal changes rows/columns instead of stretching pixels.");
            ui.add_space(8.0);
            self.show_terminal_widget(ui);
        });

        ui.ctx().request_repaint();
    }
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
            Some("parley_ratatui.egui_resizable"),
        );
        let renderer = GpuRenderer::new(&device).expect("vello renderer");
        let readback = AsyncTextureReadback::new();

        Self {
            device,
            queue,
            renderer,
            target,
            readback,
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> bool {
        if self.target.width == width && self.target.height == height {
            return false;
        }

        self.target = TextureTarget::new(
            &self.device,
            width,
            height,
            self.target.format,
            Some("parley_ratatui.egui_resizable"),
        );
        true
    }
}

fn snap_to_physical_pixels(position: egui::Pos2, pixels_per_point: f32) -> egui::Pos2 {
    egui::pos2(
        (position.x * pixels_per_point).round() / pixels_per_point,
        (position.y * pixels_per_point).round() / pixels_per_point,
    )
}

struct TerminalDemo {
    columns: u16,
    rows: u16,
    frame_count: u64,
}

impl Widget for TerminalDemo {
    fn render(self, area: Rect, buf: &mut parley_ratatui::ratatui::buffer::Buffer) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .areas(area);

        let title = format!("egui Resize widget  {}x{} cells", self.columns, self.rows);
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Ratatui Buffer", Style::new().fg(Color::LightCyan).bold()),
                Span::raw(" -> "),
                Span::styled(
                    "Parley shaping",
                    Style::new().fg(Color::LightGreen).italic(),
                ),
                Span::raw(" -> "),
                Span::styled("Vello texture", Style::new().fg(Color::LightYellow)),
                Span::raw(" -> "),
                Span::styled(
                    "egui TextureHandle",
                    Style::new().fg(Color::LightMagenta).bold(),
                ),
            ]),
            Line::from(format!(
                "cells are recalculated from widget pixels every frame; frame {}",
                self.frame_count
            )),
        ])
        .block(Block::new().title(title).borders(Borders::ALL))
        .render(header, buf);

        Paragraph::new(demo_lines())
            .block(Block::new().title("content").borders(Borders::ALL))
            .render(body, buf);

        Paragraph::new(Line::from(vec![
            Span::styled("resize behavior ", Style::new().fg(Color::Gray)),
            Span::styled(
                "no texture stretching",
                Style::new().fg(Color::LightGreen).bold(),
            ),
            Span::raw("  "),
            Span::styled("unicode ", Style::new().fg(Color::Gray)),
            Span::raw("日本語 한글 😀 box ┌─┐"),
        ]))
        .block(Block::new().borders(Borders::ALL))
        .render(footer, buf);
    }
}

fn demo_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("modifiers ", Style::new().fg(Color::Gray)),
            Span::styled("BOLD", Style::new().fg(Color::White).bold()),
            Span::raw("  "),
            Span::styled("ITALIC", Style::new().fg(Color::White).italic()),
            Span::raw("  "),
            Span::styled("UNDERLINE", Style::new().fg(Color::White).underlined()),
            Span::raw("  "),
            Span::styled(
                "REVERSED",
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightCyan)
                    .reversed(),
            ),
            Span::raw("  "),
            Span::styled(
                "ALL FLAGS",
                Style::new().fg(Color::LightGreen).add_modifier(
                    Modifier::BOLD
                        | Modifier::ITALIC
                        | Modifier::UNDERLINED
                        | Modifier::CROSSED_OUT,
                ),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("colors    ", Style::new().fg(Color::Gray)),
            Span::styled(" red ", Style::new().fg(Color::Red)),
            Span::styled(" green ", Style::new().fg(Color::Green)),
            Span::styled(" blue ", Style::new().fg(Color::Blue)),
            Span::styled(" truecolor ", Style::new().fg(Color::Rgb(255, 160, 64))),
            Span::styled(" indexed-51 ", Style::new().fg(Color::Indexed(51))),
        ]),
        Line::from(vec![
            Span::styled("unicode   ", Style::new().fg(Color::Gray)),
            Span::styled("日本語 こんにちは", Style::new().fg(Color::LightCyan)),
            Span::raw("  "),
            Span::styled("한글 표시", Style::new().fg(Color::LightMagenta)),
            Span::raw("  "),
            Span::styled("emoji 😀 🚀 ✨", Style::new().fg(Color::LightYellow)),
        ]),
        Line::from(vec![
            Span::styled("box       ", Style::new().fg(Color::Gray)),
            Span::raw("┌────┬────┐  ├────┼────┤  └────┴────┘  ░▒▓█"),
        ]),
    ]
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
