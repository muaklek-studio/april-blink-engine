// Fog shared by the forward and particle passes (prepended to both; their `Frame` carries the
// fog and sun fields used here).
//
// * Exponential height fog, integrated exactly along the view ray: thickest below the fog
//   height, thinning above it. The camera's part of the integral is worked out once per frame
//   on the CPU (`fog_scatter.z`).
// * Drifting patches: the density near the camera is modulated by a tiling noise volume that
//   the wind scrolls (`fog_wind`), sampled twice along the ray. Further away the patches
//   average out, as they do in real fog, so the far fog stays even and cheap.
// * Sun in-scattering: fog glows towards the sun (Henyey-Greenstein phase), and the glow
//   takes the sun's colour.

@group(0) @binding(6) var fog_noise: texture_3d<f32>;
@group(0) @binding(7) var fog_noise_sampler: sampler;

const FOG_PI: f32 = 3.14159265;
// Metres of the view ray over which the fog is patchy.
const FOG_PATCH_REACH: f32 = 60.0;

// Opacity of the fog between the camera and `p` (0 when fog is off).
fn fog_amount(p: vec3<f32>) -> f32 {
    let density = frame.fog_params.x;
    if frame.fog_color.w < 0.5 || density <= 0.0 {
        return 0.0;
    }
    let camera = frame.camera_position.xyz;
    let ray = p - camera;
    let distance = length(ray);
    var depth = density * distance;
    let falloff = frame.fog_params.z;
    if falloff > 0.0 {
        // ∫ exp(-falloff (y - height)) along the ray = camera term × (1 - e^-x) / x.
        let x = falloff * ray.y;
        var ratio = 1.0;
        if abs(x) > 1e-4 {
            ratio = (1.0 - exp(clamp(-x, -80.0, 80.0))) / x;
        }
        depth *= frame.fog_scatter.z * ratio;
    }
    let patchiness = frame.fog_scatter.w;
    if patchiness > 0.0 {
        depth *= fog_patches(camera, ray, distance, patchiness);
    }
    return min(1.0 - exp(-max(depth, 0.0)), frame.fog_params.w);
}

// How much denser (> 1) or thinner (< 1) than average the fog is along the ray.
fn fog_patches(camera: vec3<f32>, ray: vec3<f32>, distance: f32, amount: f32) -> f32 {
    let reach = min(distance, FOG_PATCH_REACH);
    let dir = ray / max(distance, 1e-4);
    let scale = frame.fog_wind.w;
    let offset = frame.fog_wind.xyz;
    // A coarse and a finer layer; the fine one drifts faster. The offset wraps at whole
    // units, so it is only ever scaled by whole numbers (the volume repeats every unit).
    let near = (camera + dir * reach * 0.35) * scale - offset;
    let far = (camera + dir * reach * 0.8) * (scale * 2.3) - offset * 3.0 + vec3(0.37, 0.11, 0.73);
    let a = textureSampleLevel(fog_noise, fog_noise_sampler, near, 0.0).r;
    let b = textureSampleLevel(fog_noise, fog_noise_sampler, far, 0.0).r;
    let noise = a * 0.65 + b * 0.35;
    // Clear below 0.3, twice as dense above 0.7: gaps and banks, about as much fog overall.
    let banks = 2.0 * smoothstep(0.3, 0.7, noise);
    let local = mix(1.0, banks, amount);
    // Only the first `reach` metres are patchy.
    return (local * reach + (distance - reach)) / max(distance, 1e-4);
}

// The colour of the fog seen towards `p`: its own colour, lit by the sun from behind it.
fn fog_light(p: vec3<f32>) -> vec3<f32> {
    var color = frame.fog_color.rgb;
    let strength = frame.fog_scatter.x;
    if strength > 0.0 && frame.sun_direction.w > 0.5 {
        let view = normalize(p - frame.camera_position.xyz);
        let cos_theta = dot(view, -frame.sun_direction.xyz);
        let g = frame.fog_scatter.y;
        let g2 = g * g;
        let phase = (1.0 - g2) / (4.0 * FOG_PI * pow(max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4), 1.5));
        color += frame.sun_radiance.rgb * strength * phase;
    }
    return color;
}

fn apply_fog(color: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let amount = fog_amount(p);
    if amount <= 0.0 {
        return color;
    }
    return mix(color, fog_light(p), amount);
}
