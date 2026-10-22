// Camera-facing particle quads in the HDR pass. Output is premultiplied: alpha particles
// cover what is behind them, additive ones (alpha output 0) only add light.

struct Frame {
    view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,       // rgb, w: enabled
    fog_params: vec4<f32>,      // density, height, height falloff, max opacity
};

struct View {
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> view: View;

struct Instance {
    @location(0) position_size: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) velocity_stretch: vec4<f32>,
    // rotation, hardness, additive, unused
    @location(3) params: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world: vec3<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_particle(@builtin(vertex_index) vertex: u32, instance: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(-1.0, 1.0),
        vec2(-1.0, 1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    );
    let corner = corners[vertex];
    let size = instance.position_size.w;
    var axis_x = view.right.xyz;
    var axis_y = view.up.xyz;
    var length_scale = 1.0;

    let velocity = instance.velocity_stretch.xyz;
    let stretch = instance.velocity_stretch.w;
    let in_plane = velocity - dot(velocity, view.forward.xyz) * view.forward.xyz;
    if stretch > 0.0 && length(in_plane) > 1e-4 {
        axis_y = normalize(in_plane);
        axis_x = normalize(cross(axis_y, view.forward.xyz));
        length_scale = 1.0 + stretch * length(velocity);
    } else {
        let c = cos(instance.params.x);
        let s = sin(instance.params.x);
        let x = axis_x;
        axis_x = c * x + s * axis_y;
        axis_y = -s * x + c * axis_y;
    }
    let world = instance.position_size.xyz
        + (corner.x * axis_x + corner.y * axis_y * length_scale) * (size * 0.5);

    var out: VsOut;
    out.clip = frame.view_proj * vec4(world, 1.0);
    out.uv = corner;
    out.color = instance.color;
    out.world = world;
    out.params = instance.params;
    return out;
}

fn fog_amount(world_position: vec3<f32>) -> f32 {
    if frame.fog_color.w < 0.5 {
        return 0.0;
    }
    let density = frame.fog_params.x;
    let base = frame.fog_params.y;
    let falloff = frame.fog_params.z;
    let camera = frame.camera_position.xyz;
    let distance = length(world_position - camera);
    var optical_depth = density * distance;
    if falloff > 0.0 {
        let start = exp(clamp(-falloff * (camera.y - base), -80.0, 80.0));
        let dh = world_position.y - camera.y;
        var scale = start;
        if abs(falloff * dh) > 1e-4 {
            scale = start * (1.0 - exp(clamp(-falloff * dh, -80.0, 80.0))) / (falloff * dh);
        }
        optical_depth = density * distance * scale;
    }
    return min(1.0 - exp(-max(optical_depth, 0.0)), frame.fog_params.w);
}

@fragment
fn fs_particle(in: VsOut) -> @location(0) vec4<f32> {
    let hardness = clamp(in.params.y, 0.0, 0.98);
    let d = length(in.uv);
    if d >= 1.0 {
        discard;
    }
    // Soft round profile; the hardness keeps the core solid out to that radius.
    let edge = clamp((1.0 - d) / (1.0 - hardness), 0.0, 1.0);
    let shape = edge * edge * (3.0 - 2.0 * edge);
    let alpha = clamp(in.color.a * shape, 0.0, 1.0);
    let fog = fog_amount(in.world);
    if in.params.z > 0.5 {
        // Additive: fog hides the light it adds.
        return vec4(in.color.rgb * alpha * (1.0 - fog), 0.0);
    }
    let rgb = mix(in.color.rgb, frame.fog_color.rgb, fog);
    return vec4(rgb * alpha, alpha);
}
