//! Render resources registered by game code and referenced from components.

use crate::material::Material;
use crate::mesh::{Aabb, Mesh, MeshError};
use crate::shader::{ShaderCode, ShaderHandle};
use crate::texture::{Texture, TextureError};

/// Reference to a mesh in [`RenderAssets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshHandle(pub(crate) u32);

/// Reference to a material in [`RenderAssets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MaterialHandle(pub(crate) u32);

/// Reference to a texture in [`RenderAssets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureHandle(pub(crate) u32);

macro_rules! handle_index {
    ($($t:ty),+) => {$(
        impl $t {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    )+};
}
handle_index!(MeshHandle, MaterialHandle, TextureHandle);

#[derive(Debug)]
pub(crate) struct MeshEntry {
    pub(crate) mesh: Mesh,
    pub(crate) aabb: Aabb,
    /// Per skin joint, the box of the vertices it moves (see [`crate::bounds`]).
    pub(crate) joint_bounds: Vec<Option<Aabb>>,
}

/// ECS resource holding meshes, textures and materials.
///
/// Meshes and textures are immutable once added (the renderer uploads each one once).
/// Materials can be edited at any time through [`RenderAssets::material_mut`]; they are
/// re-sent every frame. Unloading is not supported yet.
#[derive(Debug)]
pub struct RenderAssets {
    id: u64,
    meshes: Vec<MeshEntry>,
    textures: Vec<Texture>,
    materials: Vec<Material>,
    shaders: Vec<ShaderCode>,
}

impl Default for RenderAssets {
    fn default() -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            meshes: Vec::new(),
            textures: Vec::new(),
            materials: Vec::new(),
            shaders: Vec::new(),
        }
    }
}

impl RenderAssets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unique per instance: handles are only meaningful in the `RenderAssets` that issued
    /// them, so caches of handles (e.g. the asset server's) remember which one that was.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Validates and registers a mesh.
    pub fn add_mesh(&mut self, mesh: Mesh) -> Result<MeshHandle, MeshError> {
        mesh.validate()?;
        let aabb = mesh.aabb().ok_or(MeshError::Empty)?;
        let joint_bounds = crate::bounds::joint_bounds(&mesh);
        let handle = MeshHandle(self.meshes.len() as u32);
        self.meshes.push(MeshEntry {
            mesh,
            aabb,
            joint_bounds,
        });
        Ok(handle)
    }

    /// Validates and registers a texture.
    pub fn add_texture(&mut self, texture: Texture) -> Result<TextureHandle, TextureError> {
        texture.validate()?;
        let handle = TextureHandle(self.textures.len() as u32);
        self.textures.push(texture);
        Ok(handle)
    }

    pub fn add_material(&mut self, material: Material) -> MaterialHandle {
        let handle = MaterialHandle(self.materials.len() as u32);
        self.materials.push(material);
        handle
    }

    /// Registers a custom surface shader's code (see [`crate::shader`]). It is checked when
    /// the renderer builds it.
    pub fn add_shader(&mut self, name: impl Into<String>, code: impl Into<String>) -> ShaderHandle {
        let handle = ShaderHandle(self.shaders.len() as u32);
        self.shaders.push(ShaderCode {
            name: name.into(),
            code: code.into(),
            revision: 1,
        });
        handle
    }

    /// Replaces a shader's code (hot reload): materials using it are drawn with the new
    /// code from the next frame. Whether there was such a shader.
    pub fn set_shader_code(&mut self, handle: ShaderHandle, code: impl Into<String>) -> bool {
        let Some(shader) = self.shaders.get_mut(handle.index()) else {
            return false;
        };
        let code = code.into();
        if shader.code != code {
            shader.code = code;
            shader.revision += 1;
        }
        true
    }

    pub fn shader(&self, handle: ShaderHandle) -> Option<&ShaderCode> {
        self.shaders.get(handle.index())
    }

    pub fn shader_count(&self) -> usize {
        self.shaders.len()
    }

    pub fn mesh(&self, handle: MeshHandle) -> Option<&Mesh> {
        self.meshes.get(handle.index()).map(|e| &e.mesh)
    }

    /// The mesh's box in its own space (bind pose for a skinned mesh; see
    /// [`Bounds`](crate::Bounds) for the box as drawn).
    pub fn mesh_aabb(&self, handle: MeshHandle) -> Option<Aabb> {
        self.meshes.get(handle.index()).map(|e| e.aabb)
    }

    /// Per skin joint of a skinned mesh, the mesh-space box of the vertices it moves; empty
    /// for other meshes.
    pub fn mesh_joint_bounds(&self, handle: MeshHandle) -> &[Option<Aabb>] {
        self.meshes
            .get(handle.index())
            .map_or(&[], |e| &e.joint_bounds)
    }

    pub fn texture(&self, handle: TextureHandle) -> Option<&Texture> {
        self.textures.get(handle.index())
    }

    pub fn material(&self, handle: MaterialHandle) -> Option<&Material> {
        self.materials.get(handle.index())
    }

    pub fn material_mut(&mut self, handle: MaterialHandle) -> Option<&mut Material> {
        self.materials.get_mut(handle.index())
    }

    /// Bytes of the meshes' vertices, indices and skin weights (CPU copies).
    pub fn mesh_bytes(&self) -> usize {
        self.meshes
            .iter()
            .map(|e| {
                let m = &e.mesh;
                m.vertices.len() * std::mem::size_of::<crate::mesh::Vertex>()
                    + m.indices.len() * 4
                    + m.skin.as_ref().map_or(0, |s| {
                        s.len() * std::mem::size_of::<crate::mesh::SkinWeights>()
                    })
            })
            .sum()
    }

    /// Bytes of the textures' pixels (CPU copies, level 0).
    pub fn texture_bytes(&self) -> usize {
        self.textures.iter().map(|t| t.data.len()).sum()
    }

    /// Bytes of the custom shaders' code.
    pub fn shader_bytes(&self) -> usize {
        self.shaders.iter().map(|s| s.code.len()).sum()
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    pub fn material_count(&self) -> usize {
        self.materials.len()
    }

    pub(crate) fn mesh_entries(&self) -> &[MeshEntry] {
        &self.meshes
    }

    pub(crate) fn textures(&self) -> &[Texture] {
        &self.textures
    }

    pub(crate) fn materials(&self) -> &[Material] {
        &self.materials
    }

    pub(crate) fn shaders(&self) -> &[ShaderCode] {
        &self.shaders
    }
}
