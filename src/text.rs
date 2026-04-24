use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::Arc;

use parley::fontique::{FallbackKey, FamilyId, Language, ScriptExt};
use parley::layout::PositionedLayoutItem;
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight,
    GenericFamily, Layout, LayoutContext, LineHeight, StyleProperty,
};
use swash::text::Codepoint as _;

#[derive(Clone, Debug)]
pub struct FontOptions {
    pub family: String,
    pub size: f32,
    pub line_height: Option<f32>,
}

impl Default for FontOptions {
    fn default() -> Self {
        Self {
            family: String::from("FiraMono Nerd Font"),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FontVariant {
    Normal,
    Bold,
    Italic,
    BoldItalic,
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

pub(crate) struct TextSystem {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
    options: FontOptions,
    metrics: TextMetrics,
    families: Arc<[FontFamilyName<'static>]>,
    locale: Option<Language>,
    fallback_search_families: Arc<[FamilyId]>,
    checked_fallbacks: HashSet<(FallbackKey, char)>,
    fallback_family_scratch: Vec<FamilyId>,
    cache: HashMap<LayoutKey, Arc<Layout<()>>>,
}

impl TextSystem {
    pub fn new(options: FontOptions) -> Self {
        let mut font_cx = FontContext::default();
        let fallback_search_families = fallback_search_families(&mut font_cx);
        let mut text = Self {
            font_cx,
            layout_cx: LayoutContext::default(),
            families: Arc::from(font_family_stack(&options.family)),
            locale: text_locale(),
            fallback_search_families,
            checked_fallbacks: HashSet::default(),
            fallback_family_scratch: Vec::new(),
            cache: HashMap::default(),
            options,
            metrics: TextMetrics::default(),
        };
        text.metrics = text.measure_metrics();
        text
    }

    pub fn metrics(&self) -> TextMetrics {
        self.metrics
    }

    pub fn shape(&mut self, text: &str, style: TextStyle) -> Arc<Layout<()>> {
        let variant = FontVariant::from_style(style);
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
            font_size_bits: self.options.size.to_bits(),
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
            font_size_bits: self.options.size.to_bits(),
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
        let (font_style, font_weight) = font_style(variant);
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(FontFamily::from(&self.families[..]));
        builder.push_default(StyleProperty::FontSize(self.options.size));
        builder.push_default(StyleProperty::FontStyle(font_style));
        builder.push_default(StyleProperty::FontWeight(font_weight));
        builder.push_default(StyleProperty::Locale(self.locale.clone()));
        builder.push_default(LineHeight::Absolute(self.metrics.cell_height));

        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        let layout = Arc::new(layout);
        self.cache.insert(key, Arc::clone(&layout));
        layout
    }

    fn measure_metrics(&mut self) -> TextMetrics {
        let sample = "M";
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, sample, 1.0, true);
        builder.push_default(FontFamily::from(&self.families[..]));
        builder.push_default(StyleProperty::FontSize(self.options.size));
        builder.push_default(StyleProperty::Locale(self.locale.clone()));
        if let Some(line_height) = self.options.line_height {
            builder.push_default(LineHeight::Absolute(line_height));
        }

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
            cell_width: layout.full_width().floor().max(1.0),
            cell_height: line.metrics().line_height.floor().max(1.0),
            baseline: line.metrics().baseline,
            descent: line.metrics().descent,
            underline_position: run_metrics.underline_offset,
            underline_thickness: run_metrics.underline_size.max(1.0),
            strikeout_position: run_metrics.strikethrough_offset,
            strikeout_thickness: run_metrics.strikethrough_size.max(1.0),
        }
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
            .locale
            .as_ref()
            .map(|locale| FallbackKey::from((script, locale)));
        match localized {
            Some(key) if key.is_tracked() => Some(key),
            _ => Some(FallbackKey::from(script)),
        }
    }

    fn fallbacks_support_character(&mut self, key: FallbackKey, character: char) -> bool {
        let mut fallback_families = std::mem::take(&mut self.fallback_family_scratch);
        fallback_families.clear();
        fallback_families.extend(self.font_cx.collection.fallback_families(key));
        let mut buffer = [0; 4];
        let character_text = character.encode_utf8(&mut buffer);
        let supports_character = fallback_families
            .iter()
            .copied()
            .any(|family_id| self.family_supports_text(family_id, character_text));
        self.fallback_family_scratch = fallback_families;
        supports_character
    }

    fn seed_fontique_fallbacks(&mut self, key: FallbackKey, character: char) -> bool {
        let fallback_families = self.find_fallback_families(key.script(), character);
        if fallback_families.is_empty() {
            return false;
        }

        self.font_cx
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
        let Some(family) = self.font_cx.collection.family(family_id) else {
            return false;
        };

        family.fonts().iter().any(|font| {
            let Some(data) = font.load(Some(&mut self.font_cx.source_cache)) else {
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

impl FontVariant {
    fn from_style(style: TextStyle) -> Self {
        match (style.bold, style.italic) {
            (true, true) => Self::BoldItalic,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (false, false) => Self::Normal,
        }
    }
}

fn font_style(variant: FontVariant) -> (FontStyle, FontWeight) {
    match variant {
        FontVariant::Normal => (FontStyle::Normal, FontWeight::NORMAL),
        FontVariant::Bold => (FontStyle::Normal, FontWeight::BOLD),
        FontVariant::Italic => (FontStyle::Italic, FontWeight::NORMAL),
        FontVariant::BoldItalic => (FontStyle::Italic, FontWeight::BOLD),
    }
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
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
    families.push(FontFamilyName::Generic(GenericFamily::SystemUi));
    families.push(FontFamilyName::Generic(GenericFamily::Emoji));
    families
}

fn fallback_search_families(font_cx: &mut FontContext) -> Arc<[FamilyId]> {
    let mut families = Vec::new();
    let mut seen = HashSet::new();

    for generic_family in [
        GenericFamily::UiMonospace,
        GenericFamily::Monospace,
        GenericFamily::SystemUi,
        GenericFamily::Emoji,
    ] {
        for family_id in font_cx.collection.generic_families(generic_family) {
            if seen.insert(family_id) {
                families.push(family_id);
            }
        }
    }

    let mut family_names = font_cx
        .collection
        .family_names()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    family_names.sort_unstable_by_key(|family_name| family_name_sort_key(family_name));
    family_names.dedup();

    for family_name in family_names {
        let Some(family_id) = font_cx.collection.family_id(&family_name) else {
            continue;
        };
        if seen.insert(family_id) {
            families.push(family_id);
        }
    }

    Arc::from(families)
}

fn family_name_sort_key(family_name: &str) -> (bool, String) {
    (
        family_name.starts_with('.'),
        family_name.to_ascii_lowercase(),
    )
}

fn text_locale() -> Option<Language> {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| env::var(key).ok())
        .and_then(|value| normalize_locale(&value))
        .and_then(|locale| Language::parse(&locale).ok())
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
    let script = parley::fontique::Script::from_bytes(bytes);
    (!matches!(
        script,
        parley::fontique::Script::COMMON
            | parley::fontique::Script::INHERITED
            | parley::fontique::Script::UNKNOWN
    ))
    .then_some(script)
}

#[cfg(test)]
mod tests {
    use super::{FontOptions, TextSystem, fontique_script_for_char, normalize_locale};

    #[test]
    fn explicit_line_height_is_honored_without_forcing_the_default() {
        let natural = TextSystem::new(FontOptions::default()).metrics();
        let explicit = TextSystem::new(FontOptions {
            line_height: Some(64.0),
            ..FontOptions::default()
        })
        .metrics();

        assert_eq!(explicit.cell_height, 64.0);
        assert_ne!(natural.cell_height, explicit.cell_height);
    }

    #[test]
    fn common_script_symbols_do_not_seed_fontique_fallbacks() {
        assert_eq!(fontique_script_for_char('│'), None);
        assert_eq!(fontique_script_for_char('█'), None);
        assert_eq!(fontique_script_for_char(''), None);
        assert_eq!(fontique_script_for_char('!'), None);
    }

    #[test]
    fn non_common_scripts_still_seed_fontique_fallbacks() {
        assert_eq!(
            fontique_script_for_char('今'),
            Some(parley::fontique::Script::from_bytes(*b"Hani"))
        );
        assert_eq!(
            fontique_script_for_char('あ'),
            Some(parley::fontique::Script::from_bytes(*b"Hira"))
        );
        assert_eq!(
            fontique_script_for_char('한'),
            Some(parley::fontique::Script::from_bytes(*b"Hang"))
        );
    }

    #[test]
    fn locale_normalization_strips_encoding_and_uses_bcp47_separators() {
        assert_eq!(normalize_locale("ja_JP.UTF-8"), Some(String::from("ja-JP")));
        assert_eq!(
            normalize_locale("zh_Hans_CN@calendar=gregorian"),
            Some(String::from("zh-Hans-CN"))
        );
        assert_eq!(normalize_locale("C"), None);
    }
}
