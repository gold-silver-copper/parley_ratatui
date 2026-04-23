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
