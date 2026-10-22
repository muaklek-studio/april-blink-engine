// Screen-space UI: instanced quads in pixels. Output is premultiplied alpha.
//
// kind 0: solid rounded rect with border (signed distance, anti-aliased)
// kind 1: texture × color
// kind 2: MTSDF glyph (RGB = multi-channel distance, A = true distance) with outline/glow

struct Screen {
    size: vec2<f32>,
    // x: 1 when the target is not sRGB and the shader must encode gamma itself.
    flags: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) stroke_color: vec4<f32>,
    @location(4) clip: vec4<f32>,
    // corner radius, stroke width, softness, screen px range
    @location(5) params: vec4<f32>,
    @location(6) kind: u32,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Pixel position relative to the rect center.
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) stroke_color: vec4<f32>,
    @location(5) clip: vec4<f32>,
    @location(6) params: vec4<f32>,
    @location(7) @interpolate(flat) kind: u32,
};

@vertex
fn vs_ui(@builtin(vertex_index) vertex: u32, instance: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0),
    );
    let corner = corners[vertex];
    let pixel = instance.rect.xy + corner * instance.rect.zw;
    let ndc = pixel / screen.size * 2.0 - 1.0;

    var out: VsOut;
    out.position = vec4(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = mix(instance.uv.xy, instance.uv.zw, corner);
    out.half_size = instance.rect.zw * 0.5;
    out.local = (corner - 0.5) * instance.rect.zw;
    out.color = instance.color;
    out.stroke_color = instance.stroke_color;
    out.clip = instance.clip;
    out.params = instance.params;
    out.kind = instance.kind;
    return out;
}

fn rounded_rect_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + r;
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn median(v: vec3<f32>) -> f32 {
    return max(min(v.r, v.g), min(max(v.r, v.g), v.b));
}

fn premultiply(c: vec4<f32>) -> vec4<f32> {
    return vec4(c.rgb * c.a, c.a);
}

// Fill over an outer stroke: `fill` and `outer` are coverages (outer ⊇ fill).
fn fill_and_stroke(color: vec4<f32>, stroke: vec4<f32>, fill: f32, outer: f32) -> vec4<f32> {
    let f = premultiply(color) * fill;
    let s = premultiply(stroke) * max(outer - fill, 0.0);
    return f + s * (1.0 - f.a);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3(0.0031308));
}

@fragment
fn fs_ui(in: VsOut) -> @location(0) vec4<f32> {
    let color = shade(in);
    if screen.flags.x > 0.5 && color.a > 0.0 {
        return vec4(linear_to_srgb(color.rgb / color.a) * color.a, color.a);
    }
    return color;
}

fn shade(in: VsOut) -> vec4<f32> {
    let p = in.position.xy;
    if p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w {
        return vec4(0.0);
    }
    let radius = in.params.x;
    let stroke = in.params.y;
    let softness = max(in.params.z, 0.0);
    let px_range = max(in.params.w, 1.0);

    switch in.kind {
        case 1u: {
            return premultiply(textureSampleLevel(tex, samp, in.uv, 0.0) * in.color);
        }
        case 2u: {
            let texel = textureSampleLevel(tex, samp, in.uv / vec2<f32>(textureDimensions(tex)), 0.0);
            if softness > 0.0 {
                // Glow / soft shadow from the true distance (smooth beyond corners).
                let d = (texel.a - 0.5) * px_range + stroke;
                let a = smoothstep(-softness, softness * 0.25, d);
                return premultiply(in.color) * a;
            }
            let d = (median(texel.rgb) - 0.5) * px_range;
            let fill = clamp(d + 0.5, 0.0, 1.0);
            let outer = clamp(d + stroke + 0.5, 0.0, 1.0);
            return fill_and_stroke(in.color, in.stroke_color, fill, outer);
        }
        default: {
            let d = rounded_rect_distance(in.local, in.half_size, radius);
            let edge = 1.0 + softness;
            let outer = clamp(0.5 - d / edge, 0.0, 1.0);
            if stroke <= 0.0 {
                return premultiply(in.color) * outer;
            }
            let inner = clamp(0.5 - (d + stroke), 0.0, 1.0);
            return fill_and_stroke(in.color, in.stroke_color, inner, outer);
        }
    }
}
