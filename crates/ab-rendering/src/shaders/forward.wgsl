// Forward PBR pass: metallic/roughness GGX with glTF-style textures (base color,
// metallic-roughness, normal, emission), alpha cutoff, GPU skinning, directional + omni + spot
// lights, PCF shadows and fog (`fog.wgsl`, prepended). Writes linear HDR color, and the
// distance from the camera in alpha (read by the light shafts and their upsampling).

const PI: f32 = 3.14159265;
const MAX_OMNI: u32 = 16u;
const MAX_SPOT: u32 = 8u;

struct Omni {
    position_range: vec4<f32>,
    radiance: vec4<f32>,
};

struct Spot {
    position_range: vec4<f32>,
    direction_cos_outer: vec4<f32>,
    radiance_cos_inner: vec4<f32>,
    // x: shadow layer (-1 = none), y: normal offset per metre of distance
    shadow: vec4<f32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,       // rgb, w: enabled
    fog_params: vec4<f32>,      // density, height, height falloff, max opacity
    fog_scatter: vec4<f32>,     // sun scattering, anisotropy, camera height term, patchiness
    fog_wind: vec4<f32>,        // noise offset (wind × time), 1 / patch size
    sun_direction: vec4<f32>,   // xyz: travel direction, w: enabled
    sun_radiance: vec4<f32>,    // rgb, w: shadow layer (-1 = none)
    shadow_params: vec4<f32>,   // depth bias, sun normal offset (m), 1 / map size, unused
    light_counts: vec4<u32>,    // omni, spot
    shadow_matrices: array<mat4x4<f32>, 5>,
    omni: array<Omni, 16>,
    spot: array<Spot, 8>,
    inverse_view_proj: mat4x4<f32>,
    background: vec4<f32>,      // rgb
    shafts: vec4<f32>,          // light shafts: intensity, anisotropy, max distance, density
};

// The distance written for the sky and the cleared background.
const FAR_DISTANCE: f32 = 60000.0;

fn view_distance(p: vec3<f32>) -> f32 {
    return length(p - frame.camera_position.xyz);
}

struct Object {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    material: vec4<u32>,        // material index, first joint matrix, unused, unused
};

struct Material {
    albedo: vec4<f32>,
    emission: vec4<f32>,
    params: vec4<f32>,          // metallic, roughness, unshaded, alpha cutoff (0 = off)
    extra: vec4<f32>,           // normal scale, has normal map, unused, unused
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> objects: array<Object>;
@group(0) @binding(2) var<storage, read> materials: array<Material>;
@group(0) @binding(3) var shadow_map: texture_depth_2d_array;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(5) var<storage, read> joint_matrices: array<mat4x4<f32>>;

@group(1) @binding(0) var albedo_texture: texture_2d<f32>;
@group(1) @binding(1) var metallic_roughness_texture: texture_2d<f32>;
@group(1) @binding(2) var normal_texture: texture_2d<f32>;
@group(1) @binding(3) var emission_texture: texture_2d<f32>;
@group(1) @binding(4) var albedo_sampler: sampler;
@group(1) @binding(5) var metallic_roughness_sampler: sampler;
@group(1) @binding(6) var normal_sampler: sampler;
@group(1) @binding(7) var emission_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @builtin(instance_index) instance: u32,
};

struct SkinIn {
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) @interpolate(flat) material: u32,
};

fn finish_vertex(world: vec4<f32>, normal: vec3<f32>, tangent: vec4<f32>, uv: vec2<f32>, material: u32) -> VsOut {
    var out: VsOut;
    out.clip = frame.view_proj * world;
    out.world_position = world.xyz;
    out.normal = normal;
    out.tangent = tangent;
    out.uv = uv;
    out.material = material;
    return out;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let object = objects[in.instance];
    let world = object.model * vec4<f32>(in.position, 1.0);
    let normal = (object.normal * vec4<f32>(in.normal, 0.0)).xyz;
    let tangent = vec4<f32>((object.model * vec4<f32>(in.tangent.xyz, 0.0)).xyz, in.tangent.w);
    return finish_vertex(world, normal, tangent, in.uv, object.material.x);
}

// Normals transform by the inverse-transpose of the linear part: here its cofactor matrix
// (the inverse-transpose times the determinant), signed so mirrored joints keep normals
// pointing out. Right for non-uniform joint scale; the length is normalised later.
fn skin_normal_matrix(m: mat4x4<f32>) -> mat3x3<f32> {
    let a = m[0].xyz;
    let b = m[1].xyz;
    let c = m[2].xyz;
    let cofactor = mat3x3<f32>(cross(b, c), cross(c, a), cross(a, b));
    return cofactor * select(1.0, -1.0, dot(a, cross(b, c)) < 0.0);
}

@vertex
fn vs_skinned(in: VsIn, skin: SkinIn) -> VsOut {
    let object = objects[in.instance];
    let base = object.material.y;
    // Joint matrices already include the joints' global transforms (the node's own
    // transform is ignored for skinned meshes, as in glTF).
    let skin_matrix =
        joint_matrices[base + skin.joints.x] * skin.weights.x +
        joint_matrices[base + skin.joints.y] * skin.weights.y +
        joint_matrices[base + skin.joints.z] * skin.weights.z +
        joint_matrices[base + skin.joints.w] * skin.weights.w;
    let world = skin_matrix * vec4<f32>(in.position, 1.0);
    let normal = skin_normal_matrix(skin_matrix) * in.normal;
    let tangent = vec4<f32>((skin_matrix * vec4<f32>(in.tangent.xyz, 0.0)).xyz, in.tangent.w);
    return finish_vertex(world, normal, tangent, in.uv, object.material.x);
}

fn distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn geometry_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - cos_theta, 5.0);
}

// Outgoing radiance towards `v` for unit incoming radiance from `l`, including n·l.
fn brdf(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, albedo: vec3<f32>, metallic: f32, roughness: f32) -> vec3<f32> {
    let n_dot_l = max(dot(n, l), 0.0);
    if n_dot_l <= 0.0 {
        return vec3<f32>(0.0);
    }
    let h = normalize(v + l);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let f = fresnel_schlick(max(dot(h, v), 0.0), f0);
    let d = distribution_ggx(n_dot_h, roughness * roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let specular = d * g * f / (4.0 * n_dot_v * n_dot_l + 1e-4);
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metallic) * albedo / PI;
    return (diffuse + specular) * n_dot_l;
}

// Smooth window that reaches exactly zero at `range`, with inverse-square falloff inside.
fn range_attenuation(distance: f32, range: f32) -> f32 {
    let ratio = distance / range;
    let window = clamp(1.0 - ratio * ratio * ratio * ratio, 0.0, 1.0);
    return window * window / (distance * distance + 1.0);
}

// 3x3 PCF lookup. Returns 1 when lit, 0 when fully shadowed.
fn shadow_factor(layer: i32, world_position: vec3<f32>, normal: vec3<f32>, normal_offset: f32) -> f32 {
    let clip = frame.shadow_matrices[layer] * vec4<f32>(world_position + normal * normal_offset, 1.0);
    if clip.w <= 0.0 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if any(abs(ndc.xy) > vec2<f32>(1.0)) || ndc.z > 1.0 || ndc.z < 0.0 {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let depth = ndc.z - frame.shadow_params.x;
    let texel = frame.shadow_params.z;
    var lit = 0.0;
    for (var x = -1; x <= 1; x++) {
        for (var y = -1; y <= 1; y++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel;
            lit += textureSampleCompareLevel(shadow_map, shadow_sampler, uv + offset, layer, depth);
        }
    }
    return lit / 9.0;
}

@fragment
fn fs_main(in: VsOut, @builtin(front_facing) front_facing: bool) -> @location(0) vec4<f32> {
    let material = materials[in.material];
    // Sample everything up front: implicit-derivative sampling must stay in uniform control
    // flow. Missing textures are bound to 1x1 defaults (white / flat normal).
    let albedo_sample = textureSample(albedo_texture, albedo_sampler, in.uv);
    let mr_sample = textureSample(metallic_roughness_texture, metallic_roughness_sampler, in.uv);
    let normal_sample = textureSample(normal_texture, normal_sampler, in.uv).xyz;
    let emission_sample = textureSample(emission_texture, emission_sampler, in.uv).rgb;

    let base = material.albedo * albedo_sample;
    if material.params.w > 0.0 && base.a < material.params.w {
        discard;
    }
    let albedo = base.rgb;
    let emission = material.emission.rgb * emission_sample;

    if material.params.z > 0.5 {
        return vec4<f32>(apply_fog(albedo + emission, in.world_position), view_distance(in.world_position));
    }

    var n = normalize(in.normal);
    if material.extra.y > 0.5 {
        let t = normalize(in.tangent.xyz - n * dot(n, in.tangent.xyz));
        let b = cross(n, t) * in.tangent.w;
        var tn = normal_sample * 2.0 - vec3<f32>(1.0);
        tn = vec3<f32>(tn.xy * material.extra.x, tn.z);
        n = normalize(mat3x3<f32>(t, b, n) * tn);
    }
    if !front_facing {
        n = -n;
    }
    let v = normalize(frame.camera_position.xyz - in.world_position);
    let metallic = clamp(material.params.x * mr_sample.b, 0.0, 1.0);
    let roughness = clamp(material.params.y * mr_sample.g, 0.045, 1.0);

    var color = frame.ambient.rgb * albedo * (1.0 - 0.5 * metallic);

    if frame.sun_direction.w > 0.5 {
        let l = -frame.sun_direction.xyz;
        var lit = 1.0;
        let layer = i32(frame.sun_radiance.w);
        if layer >= 0 {
            lit = shadow_factor(layer, in.world_position, n, frame.shadow_params.y);
        }
        color += brdf(n, v, l, albedo, metallic, roughness) * frame.sun_radiance.rgb * lit;
    }

    for (var i = 0u; i < min(frame.light_counts.x, MAX_OMNI); i++) {
        let light = frame.omni[i];
        let to_light = light.position_range.xyz - in.world_position;
        let distance = length(to_light);
        if distance >= light.position_range.w {
            continue;
        }
        let l = to_light / max(distance, 1e-4);
        let attenuation = range_attenuation(distance, light.position_range.w);
        color += brdf(n, v, l, albedo, metallic, roughness) * light.radiance.rgb * attenuation;
    }

    for (var i = 0u; i < min(frame.light_counts.y, MAX_SPOT); i++) {
        let light = frame.spot[i];
        let to_light = light.position_range.xyz - in.world_position;
        let distance = length(to_light);
        if distance >= light.position_range.w {
            continue;
        }
        let l = to_light / max(distance, 1e-4);
        let cos_angle = dot(-l, light.direction_cos_outer.xyz);
        let cone = smoothstep(light.direction_cos_outer.w, light.radiance_cos_inner.w, cos_angle);
        if cone <= 0.0 {
            continue;
        }
        var lit = 1.0;
        let layer = i32(light.shadow.x);
        if layer >= 0 {
            lit = shadow_factor(layer, in.world_position, n, light.shadow.y * distance);
        }
        let attenuation = range_attenuation(distance, light.position_range.w) * cone * lit;
        color += brdf(n, v, l, albedo, metallic, roughness) * light.radiance_cos_inner.rgb * attenuation;
    }

    color += emission;
    return vec4<f32>(apply_fog(color, in.world_position), view_distance(in.world_position));
}

// The background seen through the fog: a full-screen triangle on the far plane, drawn where
// no geometry is, so the sky fades into the fog like distant geometry does (thin overhead,
// thick towards the horizon, glowing around the sun).

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> SkyOut {
    let ndc = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0;
    var out: SkyOut;
    out.clip = vec4<f32>(ndc, 1.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let far = frame.inverse_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    return vec4<f32>(apply_fog(frame.background.rgb, far.xyz / far.w), FAR_DISTANCE);
}

// ---------------------------------------------------------------------------------------
// Light shafts: light scattered by the air towards the camera, traced through the shadow
// maps along each view ray (half resolution; `vs_sky` covers the target). Lit stretches of
// the ray add light, shadowed ones do not: beams through windows and gaps.
// ---------------------------------------------------------------------------------------

// Full-resolution scene color, distance from the camera in alpha.
@group(2) @binding(0) var scene_color: texture_2d<f32>;

const SHAFT_STEPS: i32 = 20;
const MAX_SHAFT_SPOTS: u32 = 4u;

// 1 when `p` is lit by the light of shadow `layer` (-1: no shadow map), 0 when shadowed.
fn shaft_visibility(layer: i32, p: vec3<f32>) -> f32 {
    if layer < 0 {
        return 1.0;
    }
    let clip = frame.shadow_matrices[layer] * vec4<f32>(p, 1.0);
    if clip.w <= 0.0 {
        return 1.0;
    }
    let ndc = clip.xyz / clip.w;
    if any(abs(ndc.xy) > vec2<f32>(1.0)) || ndc.z > 1.0 || ndc.z < 0.0 {
        return 1.0;
    }
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    // No depth bias: air has no surface to shadow itself, and the surfaces' bias (metres in
    // a big light volume) would let light leak under thin roofs and walls.
    return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, layer, ndc.z);
}

// Scattering coefficient of the air at `p`: the shafts' own density plus the fog's.
fn medium_density(p: vec3<f32>) -> f32 {
    var density = frame.shafts.w;
    if frame.fog_color.w > 0.5 {
        var height = 1.0;
        if frame.fog_params.z > 0.0 {
            height = exp(clamp(-frame.fog_params.z * (p.y - frame.fog_params.y), -80.0, 80.0));
        }
        density += frame.fog_params.x * height;
    }
    return density;
}

fn phase_hg(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4), 1.5));
}

@fragment
fn fs_shafts(in: SkyOut) -> @location(0) vec4<f32> {
    let full = vec2<i32>(textureDimensions(scene_color));
    let texel = min(vec2<i32>(in.clip.xy * 2.0), full - vec2<i32>(1));
    let distance = textureLoad(scene_color, texel, 0).a;
    let camera = frame.camera_position.xyz;
    let far = frame.inverse_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - camera);
    let reach = min(distance, frame.shafts.z);
    let dt = reach / f32(SHAFT_STEPS);
    // Interleaved gradient noise: a different start per pixel turns banding into fine grain
    // that the upsampling smooths.
    let jitter = fract(52.9829189 * fract(dot(in.clip.xy, vec2<f32>(0.06711056, 0.00583715))));
    let g = frame.shafts.y;
    let sun_on = frame.sun_direction.w > 0.5;
    let sun_layer = i32(frame.sun_radiance.w);
    let sun_phase = phase_hg(dot(dir, -frame.sun_direction.xyz), g);
    let spots = min(frame.light_counts.y, MAX_SHAFT_SPOTS);
    var light = vec3<f32>(0.0);
    var optical_depth = 0.0;
    for (var i = 0; i < SHAFT_STEPS; i++) {
        let t = (f32(i) + jitter) * dt;
        let p = camera + dir * t;
        let density = medium_density(p);
        var scattered = vec3<f32>(0.0);
        if sun_on {
            scattered += frame.sun_radiance.rgb * shaft_visibility(sun_layer, p) * sun_phase;
        }
        for (var s = 0u; s < spots; s++) {
            let spot = frame.spot[s];
            let to_point = p - spot.position_range.xyz;
            let d = length(to_point);
            if d >= spot.position_range.w {
                continue;
            }
            let travel = to_point / max(d, 1e-4);
            let cone = smoothstep(spot.direction_cos_outer.w, spot.radiance_cos_inner.w, dot(travel, spot.direction_cos_outer.xyz));
            if cone <= 0.0 {
                continue;
            }
            let lit = cone * range_attenuation(d, spot.position_range.w) * shaft_visibility(i32(spot.shadow.x), p);
            scattered += spot.radiance_cos_inner.rgb * lit * phase_hg(dot(dir, -travel), g);
        }
        light += exp(-optical_depth) * density * scattered * dt;
        optical_depth += density * dt;
    }
    return vec4<f32>(light * frame.shafts.x, distance);
}
