// Presents the terminal texture 1:1 with physical pixels.
//
// A fullscreen (clip-space) quad covers the viewport; each fragment fetches its
// texel by physical pixel coordinate via `textureLoad`, so every texel maps to
// exactly one screen pixel with no resampling — the crisp presentation pattern
// from linebender/bevy_vello, specialized to a centered, pixel-aligned sub-rect
// (the terminal grid does not always fill the window).
//
// Vello writes sRGB-encoded bytes into the plain `Rgba8Unorm` storage texture
// (which cannot carry an sRGB view), so we decode to linear here before the
// sRGB framebuffer re-encodes — same as bevy_vello.
#import bevy_render::view::View
#import bevy_sprite::mesh2d_vertex_output::VertexOutput

@group(0) @binding(0) var<uniform> view: View;
@group(2) @binding(0) var terminal_texture: texture_2d<f32>;

struct Vertex {
    @location(0) position: vec3<f32>,
};

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.position, 1.0);
    return out;
}

fn linear_from_srgba(srgba: vec4<f32>) -> vec4<f32> {
    return vec4(
        select(
            srgba.rgb / 12.92,
            pow((srgba.rgb + 0.055) / 1.055, vec3(2.4)),
            srgba.rgb > vec3(0.04045),
        ),
        srgba.a,
    );
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_size = vec2<f32>(textureDimensions(terminal_texture));
    // Center the texture in the viewport, snapped to a whole physical pixel.
    let origin = view.viewport.xy + floor((view.viewport.zw - tex_size) * 0.5);
    let p = in.position.xy - origin;
    if (p.x < 0.0 || p.y < 0.0 || p.x >= tex_size.x || p.y >= tex_size.y) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return linear_from_srgba(textureLoad(terminal_texture, vec2<i32>(p), 0));
}
