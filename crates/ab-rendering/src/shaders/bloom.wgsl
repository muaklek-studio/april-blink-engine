// Bloom: a soft-threshold prefilter into a half-resolution mip chain, 13-tap downsamples
// (Jimenez, "Next Generation Post Processing in Call of Duty: Advanced Warfare") and tent
// upsamples added back up the chain. The post pass adds mip 0 to the scene.

struct Bloom {
    params: vec4<f32>,  // threshold, soft knee, upsample radius (texels), unused
};

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> bloom: Bloom;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> VsOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(source, source_sampler, uv).rgb;
}

// 13 bilinear taps weighted as five overlapping 4×4 boxes: no shimmering when things move.
fn downsample13(uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source, 0));
    let a = tap(uv + t * vec2<f32>(-2.0, -2.0));
    let b = tap(uv + t * vec2<f32>(0.0, -2.0));
    let c = tap(uv + t * vec2<f32>(2.0, -2.0));
    let d = tap(uv + t * vec2<f32>(-2.0, 0.0));
    let e = tap(uv);
    let f = tap(uv + t * vec2<f32>(2.0, 0.0));
    let g = tap(uv + t * vec2<f32>(-2.0, 2.0));
    let h = tap(uv + t * vec2<f32>(0.0, 2.0));
    let i = tap(uv + t * vec2<f32>(2.0, 2.0));
    let j = tap(uv + t * vec2<f32>(-1.0, -1.0));
    let k = tap(uv + t * vec2<f32>(1.0, -1.0));
    let l = tap(uv + t * vec2<f32>(-1.0, 1.0));
    let m = tap(uv + t * vec2<f32>(1.0, 1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625
        + (j + k + l + m) * 0.125;
}

// Keeps only what is brighter than the threshold, with a soft knee so the edge does not pop.
@fragment
fn fs_prefilter(in: VsOut) -> @location(0) vec4<f32> {
    var color = downsample13(in.uv);
    // A NaN or infinite pixel would otherwise smear over the whole screen.
    color = select(color, vec3<f32>(0.0), color != color);
    color = min(color, vec3<f32>(256.0));
    let threshold = bloom.params.x;
    let knee = max(bloom.params.y * threshold, 1e-4);
    let brightness = max(color.r, max(color.g, color.b));
    var soft = clamp(brightness - threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-4);
    let contribution = max(soft, brightness - threshold) / max(brightness, 1e-4);
    return vec4<f32>(color * contribution, 1.0);
}

@fragment
fn fs_downsample(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample13(in.uv), 1.0);
}

// 3×3 tent; blended additively onto the next larger mip.
@fragment
fn fs_upsample(in: VsOut) -> @location(0) vec4<f32> {
    let t = bloom.params.z / vec2<f32>(textureDimensions(source, 0));
    var sum = tap(in.uv) * 4.0;
    sum += (tap(in.uv + vec2<f32>(-t.x, 0.0)) + tap(in.uv + vec2<f32>(t.x, 0.0))
        + tap(in.uv + vec2<f32>(0.0, -t.y)) + tap(in.uv + vec2<f32>(0.0, t.y))) * 2.0;
    sum += tap(in.uv + vec2<f32>(-t.x, -t.y)) + tap(in.uv + vec2<f32>(t.x, -t.y))
        + tap(in.uv + vec2<f32>(-t.x, t.y)) + tap(in.uv + vec2<f32>(t.x, t.y));
    return vec4<f32>(sum / 16.0, 1.0);
}
