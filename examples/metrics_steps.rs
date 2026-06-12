//! Prints quantized cell metrics across font sizes to expose zoom steps where
//! the cell width does not change while the cell height does.

use parley_ratatui::{CellQuantization, FontOptions, TerminalRenderer, Theme};

fn main() {
    for quantization in [
        CellQuantization::Floor,
        CellQuantization::Round,
        CellQuantization::Fractional,
    ] {
        for scale in [1.0f32, 2.0] {
            println!("=== {quantization:?} at render_scale {scale} (physical px) ===");
            let mut prev: Option<(f32, f32)> = None;
            for size in 8..=30 {
                let renderer = TerminalRenderer::new_scaled(
                    FontOptions {
                        size: size as f32,
                        cell_quantization: quantization,
                        ..FontOptions::default().with_family("DejaVu Sans Mono".to_string())
                    },
                    Theme::default(),
                    scale,
                );
                let m = renderer.metrics();
                let note = match prev {
                    Some((w, h)) => {
                        let dw = m.cell_width - w;
                        let dh = m.cell_height - h;
                        if dw == 0.0 && dh != 0.0 {
                            "  <-- WIDTH STUCK (vertical-only zoom step)"
                        } else if dh == 0.0 && dw != 0.0 {
                            "  <-- HEIGHT STUCK"
                        } else {
                            ""
                        }
                    }
                    None => "",
                };
                println!(
                    "size {size:>2}: cell {w:>7.3} x {h:>7.3}{note}",
                    w = m.cell_width,
                    h = m.cell_height
                );
                prev = Some((m.cell_width, m.cell_height));
            }
        }
    }
}
