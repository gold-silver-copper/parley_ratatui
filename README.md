# parley_ratatui

`parley_ratatui` is a `ratatui` backend that keeps the terminal grid in memory,
shapes cell text through `parley`, paints the result into a `vello::Scene`, and
renders that scene with `wgpu`.

The split is intentional:

- `ParleyBackend` implements `ratatui::backend::Backend` and can build a
  `vello::Scene`.
- `TextureRenderer` renders that scene into any caller-provided
  `wgpu::TextureView`, which is the integration point for engines such as Bevy.
- `OwnedTextureRenderer` owns a device, queue, texture, and readback buffer for
  standalone rendering or tests.

```rust
use parley_ratatui::{ParleyBackend, TextureRenderer};
use ratatui::{Terminal, widgets::Paragraph};

# fn render(device: &vello::wgpu::Device, queue: &vello::wgpu::Queue, view: &vello::wgpu::TextureView) -> Result<(), Box<dyn std::error::Error>> {
let mut backend = ParleyBackend::new(80, 24, 16.0);
let mut terminal = Terminal::new(backend)?;

terminal.draw(|frame| {
    frame.render_widget(Paragraph::new("hello from ratatui"), frame.area());
})?;

let backend = terminal.backend_mut();
let scene = backend.build_scene();
let mut renderer = TextureRenderer::new(device)?;
renderer.render_to_view(device, queue, &scene, view, backend.pixel_size(), backend.clear_color())?;
# Ok(())
# }
```

For standalone output, use `OwnedTextureRenderer::render_backend_to_rgba`.

## Fonts

`BackendConfig::new` uses system monospace fallback families by default. To use a
specific primary family, set `default_family` through the builder:

```rust
use parley_ratatui::BackendConfig;

let config = BackendConfig::new(16.0)
    .with_default_family("Fira Code")
    .with_fallback_family("monospace");
```

For non-system fonts, load the font bytes yourself and call
`ParleyBackend::register_font`. Registered font family names are inserted into
the preferred stack automatically.

## Examples

Run the standalone winit/wgpu window:

```bash
cargo run --example winit_window
```

Run the Bevy sprite texture example:

```bash
cargo run --example bevy_texture --features bevy
```

The Bevy example uses `OwnedTextureRenderer` and uploads the rendered RGBA bytes
to a Bevy `Image`. A direct render-world integration can instead use
`TextureRenderer` with Bevy's `wgpu` device, queue, and a texture view.
