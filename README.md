# parley_ratatui

`parley_ratatui` is a Ratatui backend and renderer that turns a Ratatui
`Buffer` into a Vello scene or texture. It is intended for applications that
want to build terminal-style UI with Ratatui while presenting it inside a GPU
renderer, game engine, or offscreen texture pipeline.

The crate is built around three layers:

- `ParleyBackend` records Ratatui draw calls into an in-memory `Buffer`.
- `TerminalRenderer` converts that `Buffer` into a Vello `Scene`, preserving
  text shaping, modifiers, colors, cursor state, and blink state.
- `GpuRenderer` owns Vello GPU renderer state and renders the scene into a
  `wgpu::TextureView`.

The examples show a Bevy bridge that renders Ratatui UI into an offscreen Vello
texture, reads that texture back asynchronously, and updates a Bevy `Image`.

## Supported Rendering Features

The renderer is designed to preserve terminal rendering correctness for:

- Unicode grapheme clusters and combining marks
- CJK and double-width cells
- Emoji and font fallback
- Box drawing, block elements, and powerline symbols
- Ratatui foreground/background colors
- ANSI indexed colors and truecolor `Color::Rgb`
- `BOLD`, `DIM`, `ITALIC`, `UNDERLINED`, `CROSSED_OUT`, `REVERSED`, `HIDDEN`
- `SLOW_BLINK` and `RAPID_BLINK` when using elapsed-time rendering APIs
- Cursor visibility and cursor position

## Installation

Add the crate to your project:

```toml
[dependencies]
parley_ratatui = "0.1"
```

During local development in this repository, run the examples with:

```sh
cargo run --example bevy_texture
cargo run --example bevy_colors_rgb
```

## Basic Usage

Create a Ratatui terminal with `ParleyBackend`, draw widgets into it, then render
the backend buffer into a texture.

```rust
use parley_ratatui::ratatui::Terminal;
use parley_ratatui::ratatui::widgets::{Block, Borders, Paragraph};
use parley_ratatui::vello::wgpu;
use parley_ratatui::{
    FontOptions, GpuRenderer, ParleyBackend, TerminalRenderer, TextureTarget, Theme,
};

# async fn example(
#     device: wgpu::Device,
#     queue: wgpu::Queue,
# ) -> Result<(), Box<dyn std::error::Error>> {
let mut terminal = Terminal::new(ParleyBackend::new(80, 24))?;
let mut renderer = TerminalRenderer::new(FontOptions::default(), Theme::default());
let mut gpu_renderer = GpuRenderer::new(&device)?;

terminal.draw(|frame| {
    let area = frame.area();
    let widget = Paragraph::new("Hello from Ratatui")
        .block(Block::new().title("parley_ratatui").borders(Borders::ALL));
    frame.render_widget(widget, area);
})?;

let (width, height) = renderer.texture_size_for_buffer(terminal.backend().buffer());
let target = TextureTarget::new(
    &device,
    width,
    height,
    wgpu::TextureFormat::Rgba8Unorm,
    Some("terminal-ui"),
);

gpu_renderer.render_to_texture(
    &mut renderer,
    &device,
    &queue,
    &target,
    terminal.backend().buffer(),
    Some(terminal.backend().cursor_position()),
    terminal.backend().cursor_visible(),
)?;
# Ok(())
# }
```

Prefer reusing `GpuRenderer`, `TerminalRenderer`, `TextureTarget`, and readback
state across frames. The convenience methods on `TerminalRenderer` create a new
`GpuRenderer` internally and are best for simple one-shot rendering.

## Core Types

### `ParleyBackend`

`ParleyBackend` implements Ratatui's `Backend` trait and stores the latest
terminal content in memory.

```rust
let mut terminal = Terminal::new(ParleyBackend::new(120, 40))?;
```

Useful methods:

- `ParleyBackend::new(width, height)` creates a fixed-size terminal buffer.
- `backend.buffer()` returns the current Ratatui `Buffer`.
- `backend.cursor_position()` returns Ratatui's current cursor position.
- `backend.cursor_visible()` returns whether Ratatui requested the cursor.
- `backend.resize(width, height)` resizes the backing buffer.

### `TerminalRenderer`

`TerminalRenderer` owns text shaping state, layout caches, reusable Vello scene
state, and per-frame scratch data.

```rust
let mut renderer = TerminalRenderer::new(FontOptions::default(), Theme::default());
```

Useful methods:

- `metrics()` returns measured cell metrics.
- `texture_size_for_buffer(buffer)` converts a Ratatui buffer size to pixels.
- `build_scene(buffer, cursor, cursor_visible)` returns a Vello scene reference.
- `build_scene_with_elapsed(...)` also evaluates slow/rapid blink state.
- `render_to_texture(...)` is a one-shot convenience API.
- `render_to_rgba8(...)` is a one-shot blocking readback API.
- `render_to_rgba8_into(...)` writes into caller-owned `Vec<u8>` storage.
- `register_font(...)`, `register_font_data(...)`, and
  `register_font_family(...)` register bundled fonts after construction.
- `set_font_family(...)` changes the primary family and clears layout caches.

### `GpuRenderer`

`GpuRenderer` wraps Vello's GPU renderer. Reuse one instance per `wgpu::Device`.

```rust
let mut gpu_renderer = GpuRenderer::new(&device)?;
```

Useful methods:

- `render_to_texture(...)`
- `render_to_texture_with_elapsed(...)`
- `render_to_rgba8(...)`
- `render_to_rgba8_into(...)`
- `render_to_rgba8_with_elapsed(...)`
- `render_to_rgba8_with_elapsed_into(...)`

Use the `*_with_elapsed` variants when the UI contains `SLOW_BLINK` or
`RAPID_BLINK` and you want blink state to update over time.

### `TextureTarget`

`TextureTarget` owns the destination `wgpu::Texture` and `TextureView`.

```rust
let target = TextureTarget::new(
    &device,
    width,
    height,
    wgpu::TextureFormat::Rgba8Unorm,
    Some("terminal-target"),
);
```

The target texture is created with these usages:

- `TEXTURE_BINDING`
- `COPY_SRC`
- `RENDER_ATTACHMENT`
- `STORAGE_BINDING`

Readback APIs currently support `Rgba8Unorm` and `Rgba8UnormSrgb`.

### `TextureReadback`

`TextureReadback` is a reusable blocking readback helper. It reuses the staging
buffer and writes into caller-owned output storage, but it still waits for the
GPU before returning.

```rust
let mut readback = TextureReadback::new();
let mut rgba = Vec::new();

gpu_renderer.render_to_rgba8_into(
    &mut renderer,
    &mut readback,
    &device,
    &queue,
    &target,
    terminal.backend().buffer(),
    Some(terminal.backend().cursor_position()),
    terminal.backend().cursor_visible(),
    &mut rgba,
)?;
```

Use this for screenshots, tests, export, or simple integrations. For interactive
apps, prefer `AsyncTextureReadback`.

### `AsyncTextureReadback`

`AsyncTextureReadback` pipelines GPU-to-CPU texture copies. It avoids blocking
the current frame while the GPU completes the readback.

Typical frame loop:

```rust
let mut readback = AsyncTextureReadback::new();
let mut rgba = Vec::new();

// At the start of a frame, poll the oldest pending readback.
if readback.try_read_rgba8_into(&device, &mut rgba)? {
    // Upload or copy `rgba` into your destination image.
}

// Render the new frame.
gpu_renderer.render_to_texture(
    &mut renderer,
    &device,
    &queue,
    &target,
    terminal.backend().buffer(),
    Some(terminal.backend().cursor_position()),
    terminal.backend().cursor_visible(),
)?;

// Queue readback for a future frame.
let queued = readback.submit(&device, &queue, &target)?;
if !queued {
    // The small readback pipeline is full; keep displaying the previous frame.
}
```

This introduces up to one frame of latency, but it avoids a CPU/GPU
synchronization stall.

## Font Configuration

`FontOptions` controls the primary font family, size, optional line height, and
bundled font registration.

```rust
let font = FontOptions {
    family: "JetBrains Mono, Noto Color Emoji".to_string(),
    size: 18.0,
    line_height: None,
    bundled_fonts: Vec::new(),
};

let renderer = TerminalRenderer::new(font, Theme::default());
```

The family string is parsed as a CSS-style font family list. The renderer also
appends these generic fallbacks internally:

- `ui-monospace`
- `monospace`
- `system-ui`
- `emoji`

### Builder-Style Font Options

`FontOptions` has small builder-style helpers:

```rust
let font = FontOptions::default()
    .with_family("JetBrains Mono")
    .with_bundled_font_data(include_bytes!("../assets/FallbackSymbols.ttf"));
```

Available helpers:

- `with_family(family)` sets the primary family.
- `with_bundled_font(font)` registers a `BundledFont`.
- `with_bundled_font_data(data)` registers `include_bytes!`-style data.
- `with_bundled_font_family(family_name, data)` registers data under
  `family_name` and selects that family as the primary font.

### Bundled Fonts

Use `BundledFont` when you need to ship fonts with your application.

```rust
use parley_ratatui::{BundledFont, FontOptions};

let font = FontOptions::default()
    .with_family("App Mono")
    .with_bundled_font(
        BundledFont::from_static(include_bytes!("../assets/AppMono-Regular.ttf"))
            .with_family_name("App Mono"),
    );
```

For the common case, use `with_bundled_font_family`:

```rust
let font = FontOptions::default().with_bundled_font_family(
    "App Mono",
    include_bytes!("../assets/AppMono-Regular.ttf"),
);
```

`include_bytes!` uses the zero-copy static path. If you load a font file at
runtime, use `BundledFont::from_vec(bytes)`.

```rust
let bytes = std::fs::read("assets/AppMono-Regular.ttf")?;
let font = FontOptions::default()
    .with_family("App Mono")
    .with_bundled_font(BundledFont::from_vec(bytes).with_family_name("App Mono"));
```

Registering after construction is also supported:

```rust
let mut renderer = TerminalRenderer::new(FontOptions::default(), Theme::default());
let count = renderer.register_font_family(
    "App Mono",
    include_bytes!("../assets/AppMono-Regular.ttf"),
);

if count == 0 {
    eprintln!("font data did not contain a usable font");
}
```

Runtime registration clears layout caches and recomputes text metrics if at
least one font was registered.

### Font Fallback and Unicode

Parley and Fontique handle shaping and fallback. The renderer additionally seeds
fallbacks for scripts that Fontique does not already cover on the current
platform. This matters for CJK, Korean, Arabic, Devanagari, emoji, and other
non-Latin text.

For best coverage, use a font stack that includes:

- A monospace terminal font for ASCII and UI glyphs
- A CJK font if your app displays Japanese, Chinese, or Korean text
- An emoji font for emoji and emoji ZWJ sequences
- A symbol or Nerd Font if your UI uses powerline/private-use glyphs

Example:

```rust
let font = FontOptions::default()
    .with_family("App Mono, Noto Sans CJK JP, Noto Color Emoji");
```

## Theme and Color Configuration

`Theme` controls default foreground/background colors, cursor color, and the
16-color ANSI palette.

```rust
use parley_ratatui::{Rgba, Theme};

let theme = Theme {
    foreground: Rgba::rgb(230, 230, 230),
    background: Rgba::rgb(18, 18, 18),
    cursor: Rgba::rgb(255, 180, 80),
    palette: Theme::default().palette,
};
```

Ratatui styles are resolved as follows:

- `Color::Reset` and missing colors use `Theme::foreground` or
  `Theme::background`.
- Named ANSI colors use `Theme::palette`.
- `Color::Indexed(0..=15)` uses `Theme::palette`.
- `Color::Indexed(16..=231)` maps to the 6x6x6 ANSI color cube.
- `Color::Indexed(232..=255)` maps to grayscale ramp colors.
- `Color::Rgb(r, g, b)` is preserved as truecolor.
- `Modifier::DIM` dims resolved colors.
- `Modifier::REVERSED` swaps resolved foreground and background.

## Cursor and Blink

Ratatui cursor state is stored by `ParleyBackend`. Pass it into render calls:

```rust
let cursor = Some(terminal.backend().cursor_position());
let cursor_visible = terminal.backend().cursor_visible();
```

For blinking modifiers, use elapsed-time APIs:

```rust
gpu_renderer.render_to_texture_with_elapsed(
    &mut renderer,
    &device,
    &queue,
    &target,
    terminal.backend().buffer(),
    cursor,
    cursor_visible,
    elapsed_seconds,
)?;
```

`SLOW_BLINK` and `RAPID_BLINK` are resolved by hiding foreground text and
decorations during the hidden phase.

## Bevy Integration

The examples use this flow:

1. Draw Ratatui widgets into `ParleyBackend`.
2. Render the buffer into a Vello-owned offscreen `TextureTarget`.
3. Queue an asynchronous readback from that texture.
4. Copy completed readback bytes into a Bevy `Image`.

The examples intentionally borrow `terminal.backend().buffer()` directly. Avoid
cloning Ratatui buffers in frame loops.

The current Bevy examples use a separate Vello `wgpu::Device`. Bevy 0.18 and
Vello 0.8 currently use different `wgpu` versions in this dependency graph, so
passing a Vello-created `wgpu::Texture` directly into Bevy's render world is not
available through the public types used by these examples. The async readback
bridge is the optimized fallback: it reuses staging buffers and avoids blocking
on the GPU every frame, but it is still a GPU-to-CPU-to-Bevy upload path.

For a deeper Bevy integration, render on Bevy's render-world device and write
directly to a Bevy-managed texture. That requires a custom Bevy render plugin
and version-compatible `wgpu` access.

## Performance Guidance

### Runtime Practices

Prefer this in frame loops:

- Reuse `TerminalRenderer`.
- Reuse `GpuRenderer`.
- Reuse `TextureTarget` until the terminal pixel size changes.
- Reuse `TextureReadback` or `AsyncTextureReadback`.
- Use `render_to_texture` if the destination can consume a texture directly.
- Use `render_to_rgba8_into` instead of `render_to_rgba8` when you need CPU
  bytes.
- Prefer `AsyncTextureReadback` for interactive bridges.
- Borrow `terminal.backend().buffer()` directly.
- Avoid cloning Ratatui `Buffer`s.
- Avoid constructing a new `GpuRenderer` through `TerminalRenderer` convenience
  methods each frame.

Prefer this:

```rust
let buffer = terminal.backend().buffer();
gpu_renderer.render_to_texture(
    &mut renderer,
    &device,
    &queue,
    &target,
    buffer,
    Some(terminal.backend().cursor_position()),
    terminal.backend().cursor_visible(),
)?;
```

Avoid this in frame loops:

```rust
let buffer = terminal.backend().buffer().clone();
let rgba = renderer.render_to_rgba8(
    &device,
    &queue,
    &target,
    &buffer,
    Some(cursor),
    cursor_visible,
)?;
```

### Debug Build Performance

Text shaping, Vello, WGPU, and Bevy are expensive in unoptimized debug builds.
For this repository, `Cargo.toml` includes:

```toml
[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3
```

This keeps the local crate easier to debug while compiling dependencies with
optimization.

Cargo profile settings only apply from the workspace root or final binary crate.
They do not propagate from a library dependency. Downstream applications should
put the same settings in their own root `Cargo.toml` if they want similar debug
runtime performance:

```toml
[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3
```

If rebuild time matters more than debug runtime speed, consider using a custom
profile in your application instead:

```toml
[profile.dev-fast]
inherits = "dev"
opt-level = 1

[profile.dev-fast.package."*"]
opt-level = 3
```

Then run:

```sh
cargo run --profile dev-fast --example bevy_texture
```

### Profiling

For profiling, use a release-like profile with debug symbols:

```toml
[profile.profiling]
inherits = "release"
debug = true
strip = false
```

Then profile your application with its normal workload. Useful timing buckets:

- Ratatui `Terminal::draw`
- `TerminalRenderer::build_scene_with_elapsed`
- Vello `GpuRenderer::render_to_texture`
- Readback submit/poll/copy
- Destination texture or image upload

## Examples

### `bevy_texture`

Demonstrates a Unicode/style matrix rendered into a Bevy sprite. It exercises
modifiers, truecolor, CJK, emoji sequences, combining marks, box drawing, and
blink.

```sh
cargo run --example bevy_texture
```

### `bevy_colors_rgb`

Demonstrates high-frequency truecolor updates using upper-half block glyphs.
This is useful for checking whether the bridge can keep up with dense color
changes.

```sh
cargo run --example bevy_colors_rgb
```

## Limitations

- Readback APIs support `Rgba8Unorm` and `Rgba8UnormSrgb`.
- The Bevy examples use CPU image data as the bridge between Vello and Bevy.
- Runtime font registration invalidates layout caches and recomputes metrics.
- `TextureTarget` must be recreated when the terminal pixel size changes.
- The renderer assumes terminal-style cell layout. It shapes Unicode text, but
  Ratatui still owns the cell grid and cell contents.

## Development Checks

Run these before sending changes:

```sh
cargo fmt --check
cargo check --examples
cargo test
```
