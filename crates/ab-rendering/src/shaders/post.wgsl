// Full-screen post-process: camera motion blur or depth of field, light shafts (upsampled),
// chromatic aberration, bloom, exposure, ACES tonemap,
// saturation/contrast/tint, vignette, film grain, a screen overlay (flashes, fades), and sRGB
// encoding when the target format does not do it in hardware.

struct Post {
    params: vec4<f32>,    // exposure, contrast, saturation, tonemap (0/1)
    tint: vec4<f32>,
    vignette: vec4<f32>,  // intensity, smoothness, encode sRGB (0/1), light shafts (0/1)
    effects: vec4<f32>,   // chromatic aberration, grain, bloom intensity, seconds
    overlay: vec4<f32>,   // rgb, amount
    dof: vec4<f32>,       // focus distance, focus range, max blur (px at 1080 lines), on
    motion: vec4<f32>,    // strength, on
    camera: vec4<f32>,    // camera position
    inverse_view_proj: mat4x4<f32>,
    previous_view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;
@group(0) @binding(2) var<uniform> post: Post;
@group(0) @binding(3) var bloom_texture: texture_2d<f32>;
// Half resolution light shafts: rgb, and the distance they were traced to in alpha.
@group(0) @binding(4) var shafts_texture: texture_2d<f32>;

// The light shafts at a full-resolution pixel: a 3 × 3 tent of the nearest half-resolution
// samples, weighted by how close their distance is to this pixel's, which smooths the
// tracing's per-pixel jitter without bleeding beams over silhouettes.
fn shafts_at(pixel: vec2<f32>, distance: f32) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(shafts_texture));
    let position = pixel * 0.5 - vec2<f32>(0.5);
    let centre = vec2<i32>(round(position));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let at = clamp(centre + vec2<i32>(i, j), vec2<i32>(0), size - vec2<i32>(1));
            let sample = textureLoad(shafts_texture, at, 0);
            let offset = vec2<f32>(at) - position;
            let tent = max(1.5 - abs(offset.x), 0.0) * max(1.5 - abs(offset.y), 0.0);
            let closeness = 1.0 / (abs(sample.a - distance) / max(distance, 1.0) + 0.02);
            let w = tent * closeness + 1e-5;
            sum += sample.rgb * w;
            total += w;
        }
    }
    return sum / total;
}

// The HDR color and distance at `uv`, without derivatives (usable in any control flow).
fn hdr_at(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(hdr_texture, hdr_sampler, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
}

// Blur radius in pixels for something `distance` metres away.
fn circle_of_confusion(distance: f32, scale: f32) -> f32 {
    let away = abs(distance - post.dof.x);
    return clamp(away / post.dof.y, 0.0, 1.0) * post.dof.z * scale;
}

const DOF_TAPS: i32 = 24;
const GOLDEN_ANGLE: f32 = 2.39996323;

// Depth of field: a disc of taps as wide as this pixel's blur, each counting as far as its
// own blur reaches (a sharp thing in front is not smeared over by the blurred background).
fn depth_of_field(uv: vec2<f32>, centre: vec4<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(hdr_texture));
    let scale = size.y / 1080.0;
    let radius = circle_of_confusion(centre.a, scale);
    if radius < 0.5 {
        return centre.rgb;
    }
    var sum = centre.rgb;
    var total = 1.0;
    for (var i = 1; i <= DOF_TAPS; i++) {
        let r = sqrt(f32(i) / f32(DOF_TAPS)) * radius;
        let a = f32(i) * GOLDEN_ANGLE;
        let tap = hdr_at(uv + vec2<f32>(cos(a), sin(a)) * r / size);
        let reach = circle_of_confusion(tap.a, scale);
        let w = clamp(reach - r + 1.0, 0.0, 1.0);
        sum += tap.rgb * w;
        total += w;
    }
    return sum / total;
}

const MOTION_TAPS: i32 = 10;

// Camera motion blur: where this pixel's point was on screen last frame, and the colors
// along the way.
fn motion_blur(uv: vec2<f32>, centre: vec4<f32>) -> vec4<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let far = post.inverse_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - post.camera.xyz);
    let world = post.camera.xyz + dir * centre.a;
    let before = post.previous_view_proj * vec4<f32>(world, 1.0);
    if before.w <= 1e-4 {
        return vec4<f32>(centre.rgb, 0.0);
    }
    let before_uv = before.xy / before.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    var velocity = (uv - before_uv) * post.motion.x;
    // A cut or a jump is not motion.
    let speed = length(velocity);
    if speed > 0.25 {
        return vec4<f32>(centre.rgb, 0.0);
    }
    velocity = velocity * min(1.0, 0.06 / max(speed, 1e-6));
    let size = vec2<f32>(textureDimensions(hdr_texture));
    if length(velocity * size) < 1.0 {
        return vec4<f32>(centre.rgb, 0.0);
    }
    var sum = vec3<f32>(0.0);
    for (var i = 0; i < MOTION_TAPS; i++) {
        let t = f32(i) / f32(MOTION_TAPS - 1) - 0.5;
        sum += hdr_at(uv + velocity * t).rgb;
    }
    return vec4<f32>(sum / f32(MOTION_TAPS), 1.0);
}

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
    // Motion blur while the camera moves, else depth of field (both replace the sharp
    // color; chromatic aberration is kept only on a sharp image).
    var blurred = false;
    if post.motion.y > 0.5 {
        let moved = motion_blur(in.uv, hdr_at(in.uv));
        if moved.a > 0.5 {
            color = moved.rgb;
            blurred = true;
        }
    }
    if !blurred && post.dof.w > 0.5 {
        color = depth_of_field(in.uv, hdr_at(in.uv));
    }
    if post.vignette.w > 0.5 {
        let full = vec2<i32>(textureDimensions(hdr_texture));
        let pixel = min(vec2<i32>(in.clip.xy), full - vec2<i32>(1));
        let distance = textureLoad(hdr_texture, pixel, 0).a;
        color += shafts_at(in.clip.xy, distance);
    }
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
