use std::borrow::Cow;
use std::sync::Arc;

use parley::layout::PositionedLayoutItem;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight,
    GenericFamily, Layout, LayoutContext, LineHeight, StyleProperty,
};

#[derive(Clone, Debug)]
pub struct FontOptions {
    pub family: String,
    pub size: f32,
    pub line_height: Option<f32>,
}

impl Default for FontOptions {
    fn default() -> Self {
        Self {
            family: String::from("monospace"),
            size: 16.0,
            line_height: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub baseline: f32,
    pub descent: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikeout_position: f32,
    pub strikeout_thickness: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextStyle {
    pub bold: bool,
    pub italic: bool,
}

pub(crate) struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
    options: FontOptions,
    metrics: TextMetrics,
    families: Arc<[FontFamilyName<'static>]>,
}

impl TextSystem {
    pub fn new(options: FontOptions) -> Self {
        let mut text = Self {
            font_cx: FontContext::default(),
            layout_cx: LayoutContext::default(),
            families: Arc::from(font_family_stack(&options.family)),
            options,
            metrics: TextMetrics::default(),
        };
        text.metrics = text.measure_metrics();
        text
    }

    pub fn metrics(&self) -> TextMetrics {
        self.metrics
    }

    pub fn shape(&mut self, text: &str, style: TextStyle) -> Layout<()> {
        let (font_style, font_weight) = font_style(style);
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(FontFamily::from(&self.families[..]));
        builder.push_default(StyleProperty::FontSize(self.options.size));
        builder.push_default(StyleProperty::FontStyle(font_style));
        builder.push_default(StyleProperty::FontWeight(font_weight));
        builder.push_default(LineHeight::Absolute(self.metrics.cell_height));

        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }

    fn measure_metrics(&mut self) -> TextMetrics {
        let sample = "M";
        let line_height = self.options.line_height.unwrap_or(self.options.size * 1.25);
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, sample, 1.0, true);
        builder.push_default(FontFamily::from(&self.families[..]));
        builder.push_default(StyleProperty::FontSize(self.options.size));
        builder.push_default(LineHeight::Absolute(line_height));

        let mut layout = builder.build(sample);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());

        let line = layout.lines().next().expect("sample line");
        let run_metrics = line
            .items()
            .find_map(|item| match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => Some(*glyph_run.run().metrics()),
                _ => None,
            })
            .unwrap_or_default();

        TextMetrics {
            cell_width: layout.full_width().round().max(1.0),
            cell_height: line.metrics().line_height.round().max(1.0),
            baseline: line.metrics().baseline,
            descent: line.metrics().descent,
            underline_position: run_metrics.underline_offset,
            underline_thickness: run_metrics.underline_size.max(1.0),
            strikeout_position: run_metrics.strikethrough_offset,
            strikeout_thickness: run_metrics.strikethrough_size.max(1.0),
        }
    }
}

fn font_style(style: TextStyle) -> (FontStyle, FontWeight) {
    match (style.bold, style.italic) {
        (true, true) => (FontStyle::Italic, FontWeight::BOLD),
        (true, false) => (FontStyle::Normal, FontWeight::BOLD),
        (false, true) => (FontStyle::Italic, FontWeight::NORMAL),
        (false, false) => (FontStyle::Normal, FontWeight::NORMAL),
    }
}

fn font_family_stack(family: &str) -> Vec<FontFamilyName<'static>> {
    let mut families = Vec::new();
    for family in FontFamilyName::parse_css_list(family).filter_map(Result::ok) {
        match family {
            FontFamilyName::Named(name) => {
                families.push(FontFamilyName::Named(Cow::Owned(name.into_owned())))
            }
            FontFamilyName::Generic(family) => families.push(FontFamilyName::Generic(family)),
        }
    }

    if families.is_empty() {
        families.push(FontFamilyName::Named(Cow::Owned(family.to_owned())));
    }
    families.push(FontFamilyName::Generic(GenericFamily::UiMonospace));
    families.push(FontFamilyName::Generic(GenericFamily::Monospace));
    families.push(FontFamilyName::Generic(GenericFamily::Emoji));
    families
}
