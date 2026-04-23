//! A `ratatui` backend that shapes text with `parley` and renders with `vello`.
//!
//! The crate deliberately separates terminal state from GPU ownership:
//! [`ParleyBackend`] implements [`ratatui::backend::Backend`] and builds a
//! [`vello::Scene`], while [`TextureRenderer`] renders that scene into a caller
//! supplied `wgpu` texture view. Engines such as Bevy can own the device,
//! queue, and texture; standalone code can use [`OwnedTextureRenderer`].

use std::borrow::Cow;
use std::io;
use std::num::NonZeroUsize;
use std::sync::{Arc, mpsc};

use ahash::{AHashMap, AHashSet};
use parley::fontique::{Blob, FallbackKey, FamilyId};
use parley::layout::PositionedLayoutItem;
use parley::swash::text::Codepoint as _;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontStack, FontStyle as ParleyFontStyle,
    FontWeight, GenericFamily, Layout, LayoutContext, LineHeight, StyleProperty,
};
use ratatui::backend::{Backend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::style::{Color as RatatuiColor, Modifier};
use thiserror::Error;
use unicode_width::UnicodeWidthStr;
use vello::kurbo::{Affine, Rect as KRect};
use vello::peniko::{Brush, Color, Fill};
use vello::{AaConfig, AaSupport, Glyph, RenderParams, Renderer, RendererOptions, Scene, wgpu};

/// Pixel metrics for a single terminal cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    /// Width of one terminal cell in pixels.
    pub cell_width: f32,
    /// Height of one terminal cell in pixels.
    pub cell_height: f32,
    /// Baseline position in pixels from the cell origin.
    pub baseline: f32,
    /// Descent in pixels below the baseline.
    pub descent: f32,
    /// Underline offset reported by the selected font.
    pub underline_position: f32,
    /// Underline thickness reported by the selected font.
    pub underline_thickness: f32,
    /// Strikeout offset reported by the selected font.
    pub strikeout_position: f32,
    /// Strikeout thickness reported by the selected font.
    pub strikeout_thickness: f32,
    /// Additional x offset applied before painting glyphs.
    pub glyph_offset_x: f32,
    /// Additional y offset applied before painting glyphs.
    pub glyph_offset_y: f32,
}

/// Font and color configuration for [`ParleyBackend`].
#[derive(Clone, Debug)]
pub struct BackendConfig {
    /// Font family stack parsed as CSS family names.
    pub families: Vec<String>,
    /// Font size in pixels.
    pub font_size: f32,
    /// Extra horizontal pixels added to the measured cell width.
    pub cell_width_offset: f32,
    /// Extra vertical pixels added to the measured cell height.
    pub cell_height_offset: f32,
    /// Additional x offset applied before painting glyphs.
    pub glyph_offset_x: f32,
    /// Additional y offset applied before painting glyphs.
    pub glyph_offset_y: f32,
    /// Foreground color for `ratatui::style::Color::Reset`.
    pub default_fg: [u8; 3],
    /// Background color for `ratatui::style::Color::Reset`.
    pub default_bg: [u8; 3],
    /// Locale passed to parley. `None` reads `LC_ALL`, `LC_CTYPE`, then `LANG`.
    pub locale: Option<String>,
}

impl BackendConfig {
    /// Creates a config using system monospace fallback families.
    #[must_use]
    pub fn new(font_size: f32) -> Self {
        Self {
            families: Vec::new(),
            font_size,
            cell_width_offset: 0.0,
            cell_height_offset: 0.0,
            glyph_offset_x: 0.0,
            glyph_offset_y: 0.0,
            default_fg: [238, 238, 238],
            default_bg: [0, 0, 0],
            locale: text_locale(),
        }
    }

    /// Creates a config whose first preferred family is `family`.
    #[must_use]
    pub fn with_family(font_size: f32, family: impl Into<String>) -> Self {
        let mut config = Self::new(font_size);
        config.families.push(family.into());
        config
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FontVariant {
    Normal,
    Bold,
    Italic,
    BoldItalic,
}

impl FontVariant {
    const fn as_index(self) -> usize {
        match self {
            Self::Normal => 0,
            Self::Bold => 1,
            Self::Italic => 2,
            Self::BoldItalic => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum LayoutTextKey {
    Char(char),
    String(Box<str>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: LayoutTextKey,
    variant: FontVariant,
    font_size_bits: u32,
}

#[derive(Clone, Copy, Debug)]
struct ResolvedCellStyle {
    fg: [u8; 3],
    bg: [u8; 3],
    display_width: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecorationKind {
    Underline,
    Strikeout,
}

#[derive(Clone, Copy, Debug)]
struct DecorationRun {
    x: f64,
    end_x: f64,
    y: f64,
    height: f64,
    color: [u8; 3],
}

/// A ratatui backend that can build a `vello::Scene`.
pub struct ParleyBackend {
    buffer: Buffer,
    cursor_visible: bool,
    cursor_position: Position,
    config: BackendConfig,
    font_context: FontContext,
    layout_context: LayoutContext<()>,
    family_stacks: [Arc<[FontFamily<'static>]>; 4],
    fallback_search_families: Arc<[FamilyId]>,
    checked_fallbacks: AHashSet<(FallbackKey, char)>,
    cache: AHashMap<LayoutKey, Arc<Layout<()>>>,
    metrics: TextMetrics,
}

impl ParleyBackend {
    /// Creates a backend of `width` columns by `height` rows.
    #[must_use]
    pub fn new(width: u16, height: u16, font_size: f32) -> Self {
        Self::with_config(width, height, BackendConfig::new(font_size))
    }

    /// Creates a backend from a full config.
    #[must_use]
    pub fn with_config(width: u16, height: u16, config: BackendConfig) -> Self {
        let mut font_context = FontContext::default();
        let fallback_search_families = fallback_search_families(&mut font_context);
        let family_stacks = family_stacks_for_config(&config);
        let mut layout_context = LayoutContext::default();
        let metrics = measure_metrics(
            &mut font_context,
            &mut layout_context,
            &family_stacks,
            &config,
        );

        Self {
            buffer: Buffer::empty(Rect::new(0, 0, width, height)),
            cursor_visible: false,
            cursor_position: Position::ORIGIN,
            config,
            font_context,
            layout_context,
            family_stacks,
            fallback_search_families,
            checked_fallbacks: AHashSet::default(),
            cache: AHashMap::new(),
            metrics,
        }
    }

    /// Registers in-memory font data with parley and recomputes metrics.
    pub fn register_font(&mut self, font_data: &[u8]) {
        self.font_context
            .collection
            .register_fonts(Blob::new(Arc::new(font_data.to_vec())), None);
        self.fallback_search_families = fallback_search_families(&mut self.font_context);
        self.rebuild_text_state();
    }

    /// Resizes the terminal grid.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.buffer.resize(Rect::new(0, 0, width, height));
        if self.cursor_position.x >= width {
            self.cursor_position.x = width.saturating_sub(1);
        }
        if self.cursor_position.y >= height {
            self.cursor_position.y = height.saturating_sub(1);
        }
    }

    /// Returns immutable access to the current ratatui buffer.
    #[must_use]
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Returns mutable access to the current ratatui buffer.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// Returns the current text metrics.
    #[must_use]
    pub fn metrics(&self) -> TextMetrics {
        self.metrics
    }

    /// Returns the terminal texture size in pixels.
    #[must_use]
    pub fn pixel_size(&self) -> PixelSize {
        PixelSize {
            width: (self.buffer.area.width as f32 * self.metrics.cell_width)
                .ceil()
                .max(1.0) as u32,
            height: (self.buffer.area.height as f32 * self.metrics.cell_height)
                .ceil()
                .max(1.0) as u32,
        }
    }

    /// Background color used to clear the render target.
    #[must_use]
    pub fn clear_color(&self) -> Color {
        let [r, g, b] = self.config.default_bg;
        Color::from_rgb8(r, g, b)
    }

    /// Replaces the font config and clears cached layouts.
    pub fn set_config(&mut self, config: BackendConfig) {
        self.config = config;
        self.family_stacks = family_stacks_for_config(&self.config);
        self.rebuild_text_state();
    }

    /// Builds a vello scene for the current terminal buffer.
    pub fn build_scene(&mut self) -> Scene {
        let mut scene = Scene::new();
        self.paint_backgrounds(&mut scene);
        self.paint_text_and_decorations(&mut scene);
        scene
    }

    fn rebuild_text_state(&mut self) {
        self.metrics = measure_metrics(
            &mut self.font_context,
            &mut self.layout_context,
            &self.family_stacks,
            &self.config,
        );
        self.checked_fallbacks.clear();
        self.cache.clear();
    }

    fn paint_backgrounds(&self, scene: &mut Scene) {
        for y in 0..self.buffer.area.height {
            let mut run: Option<(u16, u16, [u8; 3])> = None;
            for x in 0..self.buffer.area.width {
                let style = self.resolve_buffer_cell_style(x, y);
                match run {
                    Some((start, end, color)) if color == style.bg && end == x => {
                        run = Some((start, x + 1, color));
                    }
                    _ => {
                        if let Some((start, end, color)) = run.take() {
                            self.fill_cell_run(scene, start, end, y, color);
                        }
                        run = Some((x, x + style.display_width as u16, style.bg));
                    }
                }
            }
            if let Some((start, end, color)) = run {
                self.fill_cell_run(scene, start, end, y, color);
            }
        }
    }

    fn paint_text_and_decorations(&mut self, scene: &mut Scene) {
        for y in 0..self.buffer.area.height {
            let mut underline_run = None;
            let mut strikeout_run = None;
            for x in 0..self.buffer.area.width {
                let cell = self.buffer[(x, y)].clone();
                let style = self.resolve_buffer_cell_style(x, y);
                let begin_x = x as f32 * self.metrics.cell_width;
                let begin_y = y as f32 * self.metrics.cell_height;
                let pixel_width = self.metrics.cell_width * style.display_width as f32;

                if should_shape_text(&cell) {
                    let layout = self.shape_cell(cell.symbol(), font_variant(cell.modifier));
                    self.paint_layout(scene, &layout, begin_x, begin_y, style.fg);
                }

                self.update_decoration_run(
                    scene,
                    &mut underline_run,
                    cell.modifier.contains(Modifier::UNDERLINED),
                    begin_x,
                    begin_y,
                    pixel_width,
                    style.fg,
                    DecorationKind::Underline,
                );
                self.update_decoration_run(
                    scene,
                    &mut strikeout_run,
                    cell.modifier.contains(Modifier::CROSSED_OUT),
                    begin_x,
                    begin_y,
                    pixel_width,
                    style.fg,
                    DecorationKind::Strikeout,
                );
            }
            self.flush_decoration_run(scene, &mut underline_run);
            self.flush_decoration_run(scene, &mut strikeout_run);
        }
    }

    fn fill_cell_run(&self, scene: &mut Scene, start: u16, end: u16, row: u16, color: [u8; 3]) {
        self.fill_rect(
            scene,
            start as f64 * self.metrics.cell_width as f64,
            row as f64 * self.metrics.cell_height as f64,
            end.saturating_sub(start) as f64 * self.metrics.cell_width as f64,
            self.metrics.cell_height as f64,
            color,
        );
    }

    fn resolve_buffer_cell_style(&self, x: u16, y: u16) -> ResolvedCellStyle {
        let cell = &self.buffer[(x, y)];
        let mut style = self.resolve_cell_style(cell);

        if cell.symbol() == " "
            && matches!(cell.bg, RatatuiColor::Reset)
            && x > 0
            && UnicodeWidthStr::width(self.buffer[(x - 1, y)].symbol()) > 1
        {
            let owner = self.resolve_cell_style(&self.buffer[(x - 1, y)]);
            style.fg = owner.fg;
            style.bg = owner.bg;
            style.display_width = 1;
        }

        style
    }

    fn resolve_cell_style(&self, cell: &Cell) -> ResolvedCellStyle {
        let mut fg = cell.fg;
        let bg = cell.bg;
        if cell.modifier.contains(Modifier::HIDDEN) {
            fg = bg;
        }

        let resolved_fg = ratatui_color_to_rgb(fg, self.config.default_fg);
        let resolved_bg = ratatui_color_to_rgb(bg, self.config.default_bg);
        let (mut fg, mut bg) = if cell.modifier.contains(Modifier::REVERSED) {
            (resolved_bg, resolved_fg)
        } else {
            (resolved_fg, resolved_bg)
        };

        if cell.modifier.contains(Modifier::DIM) {
            fg = dim_rgb(fg);
            bg = dim_rgb(bg);
        }

        ResolvedCellStyle {
            fg,
            bg,
            display_width: UnicodeWidthStr::width(cell.symbol()).clamp(1, 2),
        }
    }

    fn shape_cell(&mut self, text: &str, variant: FontVariant) -> Arc<Layout<()>> {
        if let Some(character) = single_char(text) {
            self.shape_char(character, variant)
        } else {
            self.shape_text(text.to_owned(), variant)
        }
    }

    fn shape_char(&mut self, character: char, variant: FontVariant) -> Arc<Layout<()>> {
        let mut buffer = [0; 4];
        let text = character.encode_utf8(&mut buffer);
        self.ensure_fontique_fallbacks(text);

        let key = LayoutKey {
            text: LayoutTextKey::Char(character),
            variant,
            font_size_bits: self.config.font_size.to_bits(),
        };
        if let Some(layout) = self.cache.get(&key) {
            return Arc::clone(layout);
        }

        self.build_and_cache_layout(key, text, variant)
    }

    fn shape_text(&mut self, text: String, variant: FontVariant) -> Arc<Layout<()>> {
        self.ensure_fontique_fallbacks(&text);

        let key = LayoutKey {
            text: LayoutTextKey::String(text.clone().into_boxed_str()),
            variant,
            font_size_bits: self.config.font_size.to_bits(),
        };
        if let Some(layout) = self.cache.get(&key) {
            return Arc::clone(layout);
        }

        self.build_and_cache_layout(key, &text, variant)
    }

    fn build_and_cache_layout(
        &mut self,
        key: LayoutKey,
        text: &str,
        variant: FontVariant,
    ) -> Arc<Layout<()>> {
        let family_stack = Arc::clone(&self.family_stacks[variant.as_index()]);
        let (font_style, font_weight) = font_style(variant);
        let mut builder =
            self.layout_context
                .ranged_builder(&mut self.font_context, text, 1.0, true);
        builder.push_default(FontStack::from(&family_stack[..]));
        builder.push_default(StyleProperty::FontSize(self.config.font_size));
        builder.push_default(StyleProperty::FontStyle(font_style));
        builder.push_default(StyleProperty::FontWeight(font_weight));
        builder.push_default(StyleProperty::Locale(self.config.locale.as_deref()));
        builder.push_default(LineHeight::Absolute(self.metrics.cell_height.max(1.0)));

        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(None, Alignment::Start, AlignmentOptions::default());

        let layout = Arc::new(layout);
        self.cache.insert(key, Arc::clone(&layout));
        layout
    }

    fn paint_layout(
        &self,
        scene: &mut Scene,
        layout: &Layout<()>,
        origin_x: f32,
        origin_y: f32,
        fg: [u8; 3],
    ) {
        let transform = Affine::translate((
            (origin_x + self.metrics.glyph_offset_x) as f64,
            (origin_y + self.metrics.glyph_offset_y) as f64,
        ));
        let brush = Brush::Solid(Color::from_rgb8(fg[0], fg[1], fg[2]));

        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };

                let run = glyph_run.run();
                let mut x = glyph_run.offset();
                let y = glyph_run.baseline();
                scene
                    .draw_glyphs(run.font())
                    .brush(&brush)
                    .hint(false)
                    .transform(transform)
                    .font_size(run.font_size())
                    .normalized_coords(run.normalized_coords())
                    .draw(
                        Fill::NonZero,
                        glyph_run
                            .glyphs()
                            .map(|glyph| scene_glyph_from_layout(&mut x, y, glyph)),
                    );
            }
        }
    }

    fn update_decoration_run(
        &self,
        scene: &mut Scene,
        active_run: &mut Option<DecorationRun>,
        enabled: bool,
        begin_x: f32,
        begin_y: f32,
        pixel_width: f32,
        color: [u8; 3],
        kind: DecorationKind,
    ) {
        if !enabled {
            self.flush_decoration_run(scene, active_run);
            return;
        }

        let (y, height) = self.decoration_geometry(begin_y, kind);
        let x = begin_x as f64;
        let end_x = (begin_x + pixel_width) as f64;

        match active_run {
            Some(run)
                if run.color == color && run.y == y && run.height == height && run.end_x == x =>
            {
                run.end_x = end_x;
            }
            _ => {
                self.flush_decoration_run(scene, active_run);
                *active_run = Some(DecorationRun {
                    x,
                    end_x,
                    y,
                    height,
                    color,
                });
            }
        }
    }

    fn flush_decoration_run(&self, scene: &mut Scene, active_run: &mut Option<DecorationRun>) {
        if let Some(run) = active_run.take() {
            self.fill_rect(
                scene,
                run.x,
                run.y,
                run.end_x - run.x,
                run.height,
                run.color,
            );
        }
    }

    fn decoration_geometry(&self, begin_y: f32, kind: DecorationKind) -> (f64, f64) {
        let (position, thickness) = match kind {
            DecorationKind::Underline => (
                self.metrics.underline_position,
                self.metrics.underline_thickness.max(1.0),
            ),
            DecorationKind::Strikeout => (
                self.metrics.strikeout_position,
                self.metrics.strikeout_thickness.max(1.0),
            ),
        };
        let y = ((begin_y + self.metrics.baseline - position) - thickness / 2.0)
            .round()
            .min(begin_y + self.metrics.cell_height - thickness);
        (y as f64, thickness as f64)
    }

    fn fill_rect(
        &self,
        scene: &mut Scene,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: [u8; 3],
    ) {
        if width <= 0.0 || height <= 0.0 {
            return;
        }

        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(color[0], color[1], color[2]),
            None,
            &KRect::new(x, y, x + width, y + height),
        );
    }

    fn ensure_fontique_fallbacks(&mut self, text: &str) {
        let mut changed = false;

        for character in text.chars() {
            let Some(key) = self.fallback_key_for_char(character) else {
                continue;
            };

            if !self.checked_fallbacks.insert((key, character)) {
                continue;
            }

            if self.fallbacks_support_character(key, character) {
                continue;
            }

            changed |= self.seed_fontique_fallbacks(key, character);
        }

        if changed {
            self.checked_fallbacks.clear();
            self.cache.clear();
        }
    }

    fn fallback_key_for_char(&self, character: char) -> Option<FallbackKey> {
        let script = fontique_script_for_char(character)?;
        let localized = self
            .config
            .locale
            .as_deref()
            .map(|locale| FallbackKey::from((script, locale)));
        match localized {
            Some(key) if key.is_tracked() => Some(key),
            _ => Some(FallbackKey::from(script)),
        }
    }

    fn fallbacks_support_character(&mut self, key: FallbackKey, character: char) -> bool {
        let fallback_families = self
            .font_context
            .collection
            .fallback_families(key)
            .collect::<Vec<_>>();
        let mut buffer = [0; 4];
        let character_text = character.encode_utf8(&mut buffer);
        fallback_families
            .into_iter()
            .any(|family_id| self.family_supports_text(family_id, character_text))
    }

    fn seed_fontique_fallbacks(&mut self, key: FallbackKey, character: char) -> bool {
        let fallback_families = self.find_fallback_families(key.script(), character);
        if fallback_families.is_empty() {
            return false;
        }

        self.font_context
            .collection
            .append_fallbacks(key, fallback_families.into_iter())
    }

    fn find_fallback_families(
        &mut self,
        script: parley::fontique::Script,
        character: char,
    ) -> Vec<FamilyId> {
        let mut character_buffer = [0; 4];
        let character_text = character.encode_utf8(&mut character_buffer);
        let sample_text = script.sample().unwrap_or(character_text);
        let use_sample_text = sample_text != character_text;
        let search_families = Arc::clone(&self.fallback_search_families);

        let mut preferred = Vec::new();
        let mut fallback_only = Vec::new();
        for &family_id in search_families.iter() {
            if !self.family_supports_text(family_id, character_text) {
                continue;
            }

            if use_sample_text && self.family_supports_text(family_id, sample_text) {
                preferred.push(family_id);
            } else {
                fallback_only.push(family_id);
            }
        }

        preferred.extend(fallback_only);
        preferred
    }

    fn family_supports_text(&mut self, family_id: FamilyId, text: &str) -> bool {
        let Some(family) = self.font_context.collection.family(family_id) else {
            return false;
        };

        family.fonts().iter().any(|font| {
            let Some(data) = font.load(Some(&mut self.font_context.source_cache)) else {
                return false;
            };
            let Some(charmap) = font.charmap_index().charmap(data.as_ref()) else {
                return false;
            };

            text.chars()
                .all(|character| charmap.map(character).is_some_and(|glyph_id| glyph_id != 0))
        })
    }
}

impl Backend for ParleyBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (x, y, cell) in content {
            if x < self.buffer.area.width && y < self.buffer.area.height {
                self.buffer[(x, y)] = cell.clone();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor_position)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.cursor_position = position.into();
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.buffer.reset();
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        Ok(Size::new(self.buffer.area.width, self.buffer.area.height))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let pixel_size = self.pixel_size();
        Ok(WindowSize {
            columns_rows: Size::new(self.buffer.area.width, self.buffer.area.height),
            pixels: Size::new(
                pixel_size.width.min(u16::MAX as u32) as u16,
                pixel_size.height.min(u16::MAX as u32) as u16,
            ),
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Pixel dimensions for a render target.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PixelSize {
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// Errors returned by the wgpu/vello render helpers.
#[derive(Debug, Error)]
pub enum RenderError {
    /// wgpu could not find an adapter.
    #[error("wgpu adapter request failed: {0}")]
    AdapterRequest(#[from] wgpu::RequestAdapterError),
    /// wgpu could not create a device.
    #[error("wgpu device request failed: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),
    /// vello could not create a renderer.
    #[error("vello renderer init failed: {0}")]
    RendererInit(vello::Error),
    /// vello failed while rendering a scene.
    #[error("vello render failed: {0}")]
    Render(vello::Error),
    /// wgpu polling failed.
    #[error("wgpu device poll failed: {0}")]
    DevicePoll(wgpu::PollError),
    /// wgpu map_async failed.
    #[error("wgpu map_async failed: {0}")]
    BufferMap(wgpu::BufferAsyncError),
    /// The map callback channel closed before returning a result.
    #[error("wgpu map_async callback channel closed")]
    MapChannelClosed,
}

/// Renders vello scenes into a caller-owned `wgpu::TextureView`.
pub struct TextureRenderer {
    renderer: Renderer,
    antialiasing_method: AaConfig,
}

impl TextureRenderer {
    /// Creates a renderer for the supplied device.
    pub fn new(device: &wgpu::Device) -> Result<Self, RenderError> {
        let renderer = Renderer::new(
            device,
            RendererOptions {
                antialiasing_support: AaSupport::all(),
                #[cfg(target_os = "macos")]
                num_init_threads: NonZeroUsize::new(1),
                #[cfg(not(target_os = "macos"))]
                num_init_threads: None,
                ..RendererOptions::default()
            },
        )
        .map_err(RenderError::RendererInit)?;

        Ok(Self {
            renderer,
            antialiasing_method: AaConfig::Msaa8,
        })
    }

    /// Sets the antialiasing method used for subsequent renders.
    pub fn set_antialiasing_method(&mut self, method: AaConfig) {
        self.antialiasing_method = method;
    }

    /// Renders `scene` into `target_view`.
    pub fn render_to_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        target_view: &wgpu::TextureView,
        size: PixelSize,
        base_color: Color,
    ) -> Result<(), RenderError> {
        self.renderer
            .render_to_texture(
                device,
                queue,
                scene,
                target_view,
                &RenderParams {
                    base_color,
                    width: size.width.max(1),
                    height: size.height.max(1),
                    antialiasing_method: self.antialiasing_method,
                },
            )
            .map_err(RenderError::Render)
    }
}

/// Standalone renderer that owns a wgpu device, queue, texture, and readback buffer.
pub struct OwnedTextureRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    readback: wgpu::Buffer,
    padded_bytes_per_row: u32,
    size: PixelSize,
    rgba: Vec<u8>,
    renderer: TextureRenderer,
}

impl OwnedTextureRenderer {
    /// Creates a standalone renderer and its backing texture.
    pub fn new(size: PixelSize) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::default();
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
        let maybe_features = wgpu::Features::CLEAR_TEXTURE | wgpu::Features::PIPELINE_CACHE;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("parley_ratatui.device"),
                required_features: adapter.features() & maybe_features,
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::default(),
                experimental_features: Default::default(),
            }))?;
        let renderer = TextureRenderer::new(&device)?;
        let size = PixelSize {
            width: size.width.max(1),
            height: size.height.max(1),
        };
        let (texture, texture_view, readback, padded_bytes_per_row) =
            create_target_resources(&device, size);

        Ok(Self {
            device,
            queue,
            texture,
            texture_view,
            readback,
            padded_bytes_per_row,
            rgba: vec![0; size.width as usize * size.height as usize * 4],
            size,
            renderer,
        })
    }

    /// Returns the owned device.
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Returns the owned queue.
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Returns the owned texture.
    #[must_use]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Returns the owned texture view.
    #[must_use]
    pub fn texture_view(&self) -> &wgpu::TextureView {
        &self.texture_view
    }

    /// Resizes the owned texture and readback buffer.
    pub fn resize(&mut self, size: PixelSize) {
        let size = PixelSize {
            width: size.width.max(1),
            height: size.height.max(1),
        };
        if self.size == size {
            return;
        }

        let (texture, texture_view, readback, padded_bytes_per_row) =
            create_target_resources(&self.device, size);
        self.texture = texture;
        self.texture_view = texture_view;
        self.readback = readback;
        self.padded_bytes_per_row = padded_bytes_per_row;
        self.size = size;
        self.rgba
            .resize(size.width as usize * size.height as usize * 4, 0);
    }

    /// Renders `backend` into the owned texture.
    pub fn render_backend(&mut self, backend: &mut ParleyBackend) -> Result<(), RenderError> {
        let size = backend.pixel_size();
        self.resize(size);
        let scene = backend.build_scene();
        self.renderer.render_to_view(
            &self.device,
            &self.queue,
            &scene,
            &self.texture_view,
            size,
            backend.clear_color(),
        )
    }

    /// Renders `backend` and returns tightly packed RGBA8 bytes.
    pub fn render_backend_to_rgba(
        &mut self,
        backend: &mut ParleyBackend,
    ) -> Result<&[u8], RenderError> {
        self.render_backend(backend)?;
        self.read_rgba()
    }

    /// Reads the current texture contents as tightly packed RGBA8 bytes.
    pub fn read_rgba(&mut self) -> Result<&[u8], RenderError> {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("parley_ratatui.readback"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.size.height),
                },
            },
            wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        let slice = self.readback.slice(..);
        let (sender, receiver) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(RenderError::DevicePoll)?;
        receiver
            .recv()
            .map_err(|_| RenderError::MapChannelClosed)?
            .map_err(RenderError::BufferMap)?;

        let mapped = slice.get_mapped_range();
        let row_len = self.size.width as usize * 4;
        self.rgba.resize(row_len * self.size.height as usize, 0);
        for y in 0..self.size.height as usize {
            let src = y * self.padded_bytes_per_row as usize;
            let dst = y * row_len;
            self.rgba[dst..dst + row_len].copy_from_slice(&mapped[src..src + row_len]);
        }
        drop(mapped);
        self.readback.unmap();
        Ok(&self.rgba)
    }
}

fn create_target_resources(
    device: &wgpu::Device,
    size: PixelSize,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Buffer, u32) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("parley_ratatui.texture"),
        size: wgpu::Extent3d {
            width: size.width,
            height: size.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let padded_bytes_per_row = align_to(size.width * 4, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parley_ratatui.readback"),
        size: padded_bytes_per_row as u64 * size.height as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (texture, texture_view, readback, padded_bytes_per_row)
}

fn measure_metrics(
    font_context: &mut FontContext,
    layout_context: &mut LayoutContext<()>,
    family_stacks: &[Arc<[FontFamily<'static>]>; 4],
    config: &BackendConfig,
) -> TextMetrics {
    let mut metrics = TextMetrics::default();

    for variant in [
        FontVariant::Normal,
        FontVariant::Bold,
        FontVariant::Italic,
        FontVariant::BoldItalic,
    ] {
        let sample = "M";
        let family_stack = Arc::clone(&family_stacks[variant.as_index()]);
        let (font_style, font_weight) = font_style(variant);
        let mut builder = layout_context.ranged_builder(font_context, sample, 1.0, true);
        builder.push_default(FontStack::from(&family_stack[..]));
        builder.push_default(StyleProperty::FontSize(config.font_size));
        builder.push_default(StyleProperty::FontStyle(font_style));
        builder.push_default(StyleProperty::FontWeight(font_weight));
        builder.push_default(StyleProperty::Locale(config.locale.as_deref()));

        let mut layout = builder.build(sample);
        layout.break_all_lines(None);
        layout.align(None, Alignment::Start, AlignmentOptions::default());

        let Some(line) = layout.lines().next() else {
            continue;
        };
        let run_metrics = line
            .items()
            .find_map(|item| match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => Some(*glyph_run.run().metrics()),
                _ => None,
            })
            .unwrap_or_default();

        metrics.cell_width = metrics.cell_width.max(layout.full_width().floor().max(1.0));
        metrics.cell_height = metrics
            .cell_height
            .max(line.metrics().line_height.floor().max(1.0));
        metrics.baseline = metrics.baseline.max(line.metrics().baseline);
        metrics.descent = metrics.descent.max(line.metrics().descent);
        metrics.underline_position = metrics.underline_position.max(run_metrics.underline_offset);
        metrics.underline_thickness = metrics
            .underline_thickness
            .max(run_metrics.underline_size.max(1.0));
        metrics.strikeout_position = metrics
            .strikeout_position
            .max(run_metrics.strikethrough_offset);
        metrics.strikeout_thickness = metrics
            .strikeout_thickness
            .max(run_metrics.strikethrough_size.max(1.0));
    }

    metrics.cell_width = (metrics.cell_width + config.cell_width_offset)
        .floor()
        .max(1.0);
    metrics.cell_height = (metrics.cell_height + config.cell_height_offset)
        .floor()
        .max(1.0);
    metrics.glyph_offset_x = config.glyph_offset_x;
    metrics.glyph_offset_y = config.glyph_offset_y;
    metrics
}

fn family_stacks_for_config(config: &BackendConfig) -> [Arc<[FontFamily<'static>]>; 4] {
    std::array::from_fn(|_| Arc::from(font_family_stack(config)))
}

fn font_family_stack(config: &BackendConfig) -> Vec<FontFamily<'static>> {
    let mut families = Vec::new();
    for family in &config.families {
        push_configured_family_names(&mut families, family);
    }
    push_family_name(&mut families, GenericFamily::UiMonospace.into());
    push_family_name(&mut families, GenericFamily::Monospace.into());
    push_family_name(&mut families, GenericFamily::SystemUi.into());
    push_family_name(&mut families, GenericFamily::Emoji.into());
    families
}

fn push_configured_family_names(families: &mut Vec<FontFamily<'static>>, spec: impl AsRef<str>) {
    let spec = spec.as_ref().trim();
    if spec.is_empty() {
        return;
    }

    let parsed = FontFamily::parse_list(spec).collect::<Vec<_>>();
    if parsed.is_empty() {
        push_family_name(families, named_family(spec));
    } else {
        for family in parsed {
            match family {
                FontFamily::Named(name) => push_family_name(families, named_family(name.as_ref())),
                FontFamily::Generic(family) => push_family_name(families, family.into()),
            }
        }
    }
}

fn named_family(name: impl AsRef<str>) -> FontFamily<'static> {
    FontFamily::Named(Cow::Owned(name.as_ref().to_owned()))
}

fn push_family_name(families: &mut Vec<FontFamily<'static>>, family: FontFamily<'static>) {
    if !families.contains(&family) {
        families.push(family);
    }
}

fn fallback_search_families(font_context: &mut FontContext) -> Arc<[FamilyId]> {
    let mut families = Vec::new();
    let mut seen = AHashSet::default();

    for generic_family in [
        GenericFamily::UiMonospace,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
        GenericFamily::Emoji,
    ] {
        for family_id in font_context.collection.generic_families(generic_family) {
            if seen.insert(family_id) {
                families.push(family_id);
            }
        }
    }

    let mut family_names = font_context
        .collection
        .family_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    family_names.sort_unstable_by_key(|family_name| {
        (
            family_name.starts_with('.'),
            family_name.to_ascii_lowercase(),
        )
    });
    family_names.dedup();

    for family_name in family_names {
        let Some(family_id) = font_context.collection.family_id(&family_name) else {
            continue;
        };
        if seen.insert(family_id) {
            families.push(family_id);
        }
    }

    Arc::from(families)
}

fn should_shape_text(cell: &Cell) -> bool {
    !cell.modifier.contains(Modifier::HIDDEN)
        && cell
            .symbol()
            .chars()
            .any(|character| !character.is_whitespace())
}

fn font_variant(modifier: Modifier) -> FontVariant {
    match (
        modifier.contains(Modifier::BOLD),
        modifier.contains(Modifier::ITALIC),
    ) {
        (true, true) => FontVariant::BoldItalic,
        (true, false) => FontVariant::Bold,
        (false, true) => FontVariant::Italic,
        (false, false) => FontVariant::Normal,
    }
}

fn font_style(variant: FontVariant) -> (ParleyFontStyle, FontWeight) {
    match variant {
        FontVariant::Normal => (ParleyFontStyle::Normal, FontWeight::NORMAL),
        FontVariant::Bold => (ParleyFontStyle::Normal, FontWeight::BOLD),
        FontVariant::Italic => (ParleyFontStyle::Italic, FontWeight::NORMAL),
        FontVariant::BoldItalic => (ParleyFontStyle::Italic, FontWeight::BOLD),
    }
}

fn scene_glyph_from_layout(
    cursor_x: &mut f32,
    baseline: f32,
    glyph: parley::layout::Glyph,
) -> Glyph {
    let positioned = Glyph {
        id: glyph.id as u32,
        x: *cursor_x + glyph.x,
        y: baseline - glyph.y,
    };
    *cursor_x += glyph.advance;
    positioned
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn ratatui_color_to_rgb(color: RatatuiColor, default: [u8; 3]) -> [u8; 3] {
    match color {
        RatatuiColor::Reset => default,
        RatatuiColor::Black => [0, 0, 0],
        RatatuiColor::Red => [205, 49, 49],
        RatatuiColor::Green => [13, 188, 121],
        RatatuiColor::Yellow => [229, 229, 16],
        RatatuiColor::Blue => [36, 114, 200],
        RatatuiColor::Magenta => [188, 63, 188],
        RatatuiColor::Cyan => [17, 168, 205],
        RatatuiColor::Gray => [229, 229, 229],
        RatatuiColor::DarkGray => [102, 102, 102],
        RatatuiColor::LightRed => [241, 76, 76],
        RatatuiColor::LightGreen => [35, 209, 139],
        RatatuiColor::LightYellow => [245, 245, 67],
        RatatuiColor::LightBlue => [59, 142, 234],
        RatatuiColor::LightMagenta => [214, 112, 214],
        RatatuiColor::LightCyan => [41, 184, 219],
        RatatuiColor::White => [255, 255, 255],
        RatatuiColor::Rgb(r, g, b) => [r, g, b],
        RatatuiColor::Indexed(index) => indexed_color_to_rgb(index),
    }
}

fn indexed_color_to_rgb(index: u8) -> [u8; 3] {
    const ANSI: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];

    match index {
        0..=15 => ANSI[index as usize],
        16..=231 => {
            let index = index - 16;
            let r = index / 36;
            let g = (index % 36) / 6;
            let b = index % 6;
            [
                color_cube_value(r),
                color_cube_value(g),
                color_cube_value(b),
            ]
        }
        232..=255 => {
            let value = 8 + (index - 232) * 10;
            [value, value, value]
        }
    }
}

fn color_cube_value(component: u8) -> u8 {
    if component == 0 {
        0
    } else {
        55 + component * 40
    }
}

fn dim_rgb([r, g, b]: [u8; 3]) -> [u8; 3] {
    [r / 2, g / 2, b / 2]
}

fn text_locale() -> Option<String> {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| std::env::var(key).ok())
        .and_then(|value| normalize_locale(&value))
}

fn normalize_locale(locale: &str) -> Option<String> {
    let locale = locale.trim();
    if locale.is_empty() || matches!(locale, "C" | "POSIX") {
        return None;
    }

    let locale = locale
        .split_once('.')
        .map(|(locale, _)| locale)
        .unwrap_or(locale);
    let locale = locale
        .split_once('@')
        .map(|(locale, _)| locale)
        .unwrap_or(locale);
    let locale = locale.replace('_', "-");

    (!locale.is_empty()).then_some(locale)
}

fn fontique_script_for_char(character: char) -> Option<parley::fontique::Script> {
    let tag = character.script().to_opentype();
    let mut bytes = [
        (tag >> 24) as u8,
        (tag >> 16) as u8,
        (tag >> 8) as u8,
        tag as u8,
    ];
    bytes[0] = bytes[0].to_ascii_uppercase();
    bytes[1] = bytes[1].to_ascii_lowercase();
    bytes[2] = bytes[2].to_ascii_lowercase();
    bytes[3] = bytes[3].to_ascii_lowercase();
    let script = parley::fontique::Script(bytes);
    (!matches!(&script.0, b"Zyyy" | b"Zinh" | b"Zzzz")).then_some(script)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::style::Stylize;
    use ratatui::widgets::Paragraph;

    use super::*;

    #[test]
    fn implements_ratatui_backend_and_keeps_buffer() {
        let mut terminal = Terminal::new(ParleyBackend::new(8, 2, 14.0)).unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new("hi").red().on_blue(), frame.area());
            })
            .unwrap();

        let backend = terminal.backend();
        assert_eq!(backend.buffer()[(0, 0)].symbol(), "h");
        assert_eq!(backend.buffer()[(1, 0)].symbol(), "i");
        assert_eq!(backend.size().unwrap(), Size::new(8, 2));
        assert!(backend.metrics().cell_width >= 1.0);
    }

    #[test]
    fn pixel_size_uses_measured_cell_metrics() {
        let backend = ParleyBackend::new(3, 2, 14.0);

        assert_eq!(
            backend.pixel_size(),
            PixelSize {
                width: (backend.metrics.cell_width * 3.0).ceil() as u32,
                height: (backend.metrics.cell_height * 2.0).ceil() as u32,
            }
        );
    }

    #[test]
    fn normalizes_locale() {
        assert_eq!(normalize_locale("ja_JP.UTF-8"), Some(String::from("ja-JP")));
        assert_eq!(normalize_locale("C"), None);
    }
}
