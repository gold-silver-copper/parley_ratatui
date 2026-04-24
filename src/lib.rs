//! A Ratatui backend that renders through Parley and Vello.
//!
//! `ParleyBackend` records Ratatui's cell updates in memory. `TerminalRenderer`
//! turns that buffer into a Vello scene and can render the scene into any
//! compatible `wgpu::Texture`.

mod backend;
mod color;
mod renderer;
mod text;

pub use backend::ParleyBackend;
pub use color::{Rgba, Theme};
pub use renderer::{GpuRenderer, RenderError, TerminalRenderer, TextureTarget};
pub use text::{FontOptions, TextMetrics};

pub use ratatui;
pub use vello;
