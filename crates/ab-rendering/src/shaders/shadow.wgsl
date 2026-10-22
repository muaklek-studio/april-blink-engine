// Depth-only pass rendering shadow casters from a light's point of view.

struct Object {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    material: vec4<u32>,
};

struct ShadowPass {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> pass_data: ShadowPass;
@group(0) @binding(1) var<storage, read> objects: array<Object>;
@group(0) @binding(2) var<storage, read> joint_matrices: array<mat4x4<f32>>;

@vertex
fn vs_shadow(@location(0) position: vec3<f32>, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    return pass_data.view_proj * objects[instance].model * vec4<f32>(position, 1.0);
}

@vertex
fn vs_shadow_skinned(
    @location(0) position: vec3<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
    @builtin(instance_index) instance: u32,
) -> @builtin(position) vec4<f32> {
    let base = objects[instance].material.y;
    let skin_matrix =
        joint_matrices[base + joints.x] * weights.x +
        joint_matrices[base + joints.y] * weights.y +
        joint_matrices[base + joints.z] * weights.z +
        joint_matrices[base + joints.w] * weights.w;
    return pass_data.view_proj * skin_matrix * vec4<f32>(position, 1.0);
}
