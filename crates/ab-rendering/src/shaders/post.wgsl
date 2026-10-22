// Full-screen post-process: chromatic aberration, bloom, exposure, ACES tonemap,
// saturation/contrast/tint, vignette, film grain, a screen overlay (flashes, fades), and sRGB
// encoding when the target format does not do it in hardware.

struct Post {
    params: vec4<f32>,    // exposure, contrast, saturation, tonemap (0/1)
    tint: vec4<f32>,
    vignette: vec4<f32>,  // intensity, smoothness, encode sRGB (0/1), unused
    effects: vec4<f32>,   // chromatic aberration, grain, bloom intensity, seconds
    overlay: vec4<f32>,   // rgb, amount
};

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;
@group(0) @binding(2) var<uniform> post: Post;
@group(0) @binding(3) var bloom_texture: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> VsOut {
    // One triangle covering the screen: (0,0), (2,0), (0,2) in UV space.
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.clip = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

// Krzysztof Narkowicz's ACES filmic fit.
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let low = c * 12.92;
    let high = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(high, low, c <= vec3<f32>(0.0031308));
}

// PCG hash (Jarzynski & Olano): stable white noise without sin() precision artefacts.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

@fragment
fn fs_post(in: VsOut) -> @location(0) vec4<f32> {
    // Red and blue sampled further out than green, growing towards the edges.
    let offset = (in.uv - vec2<f32>(0.5)) * post.effects.x * 0.012;
    var color = vec3<f32>(
        textureSample(hdr_texture, hdr_sampler, in.uv - offset).r,
        textureSample(hdr_texture, hdr_sampler, in.uv).g,
        textureSample(hdr_texture, hdr_sampler, in.uv + offset).b,
    );
    color += textureSample(bloom_texture, hdr_sampler, in.uv).rgb * post.effects.z;
    color *= post.params.x;
    if post.params.w > 0.5 {
        color = aces(color);
    } else {
        color = clamp(color, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = mix(vec3<f32>(luma), color, post.params.z);
    color = (color - vec3<f32>(0.18)) * post.params.y + vec3<f32>(0.18);
    color = clamp(color * post.tint.rgb, vec3<f32>(0.0), vec3<f32>(1.0));

    // 0 at the centre, 1 at the corners.
    let radius = distance(in.uv, vec2<f32>(0.5)) * 1.41421356;
    let smoothness = clamp(post.vignette.y, 0.01, 1.0);
    color *= 1.0 - post.vignette.x * smoothstep(1.0 - smoothness, 1.0, radius);

    if post.effects.y > 0.0 {
        let pixel = vec2<u32>(in.clip.xy);
        let frame = u32(post.effects.w * 24.0);
        let noise = f32(pcg(pixel.x + pcg(pixel.y + pcg(frame))) & 0xffffu) / 65535.0;
        // Strongest in the mid-tones, like film.
        let weight = 1.0 - abs(luma * 2.0 - 1.0) * 0.5;
        color = clamp(color + (noise - 0.5) * post.effects.y * 0.15 * weight, vec3<f32>(0.0),
            vec3<f32>(1.0));
    }

    color = mix(color, post.overlay.rgb, post.overlay.a);

    if post.vignette.z > 0.5 {
        color = linear_to_srgb(color);
    }
    return vec4<f32>(color, 1.0);
}
