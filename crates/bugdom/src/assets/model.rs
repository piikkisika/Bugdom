//! 3DMF model files (`Models/*.3dmf`), loaded as a [`Model`] whose groups
//! are the object types the game indexes by number.

use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader};
use bevy::image::ImageAddressMode;
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bugdom_formats::skin::SkinnedMesh;
use bugdom_formats::tdmf::{self, MetaFile, TexturingMode, TriMesh, UvBoundary};

pub(super) fn plugin(app: &mut App) {
    app.init_asset::<Model>().init_asset_loader::<ModelLoader>();
}

/// A loaded 3DMF file.
#[derive(Asset, TypePath, Debug)]
pub struct Model {
    /// Every mesh of the file with its material, in file order.
    pub parts: Vec<ModelPart>,
    /// The file's top-level groups. In the level model files, a group's index
    /// is the object type (`gObjectGroupList[modelFile][objectType]`).
    pub groups: Vec<ModelGroup>,
}

/// One mesh and its material.
#[derive(Debug, Clone)]
pub struct ModelPart {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

/// One object of a model file.
#[derive(Debug, Clone)]
pub struct ModelGroup {
    /// Indices into [`Model::parts`].
    pub parts: Vec<usize>,
    /// Bounds of the object's meshes (`gObjectGroupBBoxList`).
    pub aabb: bevy::camera::primitives::Aabb,
    /// Radius of the object's bounding sphere (`gObjectGroupRadiusList`).
    pub radius: f32,
}

/// Loads `.3dmf` files as [`Model`]s.
///
/// Sub-assets are labelled `Mesh{n}`, `Material{n}` and `Texture{n}`.
#[derive(Default, TypePath)]
pub struct ModelLoader;

impl AssetLoader for ModelLoader {
    type Asset = Model;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Model, BevyError> {
        let bytes = super::read_all(reader).await?;
        let file = tdmf::parse(&bytes)?;
        let parts = add_parts(&file, None, false, load_context);
        let groups = file
            .groups
            .iter()
            .map(|group| {
                let bbox = tdmf::BoundingBox::of_meshes(file.group_meshes(group));
                let sphere = tdmf::BoundingSphere::of_meshes(file.group_meshes(group));
                ModelGroup {
                    parts: group.meshes.clone(),
                    aabb: bevy::camera::primitives::Aabb::from_min_max(
                        Vec3::from(bbox.min),
                        Vec3::from(bbox.max),
                    ),
                    radius: sphere.radius,
                }
            })
            .collect();
        Ok(Model { parts, groups })
    }

    fn extensions(&self) -> &[&str] {
        &["3dmf"]
    }
}

/// Adds a mesh and material sub-asset for every mesh of `file` (and a texture
/// sub-asset for every texture), replacing positions and normals and adding
/// joints where `skin` is given.
///
/// `force_clamp` clamps every texture's UVs, as the original does for
/// skeleton models (`LoadBonesReferenceModel` in original/src/Skeleton/Bones.c).
pub(crate) fn add_parts(
    file: &MetaFile,
    skin: Option<&[SkinnedMesh]>,
    force_clamp: bool,
    load_context: &mut LoadContext,
) -> Vec<ModelPart> {
    let address_mode = |boundary: UvBoundary| match boundary {
        _ if force_clamp => ImageAddressMode::ClampToEdge,
        UvBoundary::Clamp => ImageAddressMode::ClampToEdge,
        UvBoundary::Wrap => ImageAddressMode::Repeat,
    };
    let textures: Vec<Handle<Image>> = file
        .textures
        .iter()
        .enumerate()
        .map(|(i, texture)| {
            let image = super::image::rgba8_image(
                texture.pixmap.width,
                texture.pixmap.height,
                texture.pixmap.to_rgba8(),
                address_mode(texture.boundary_u),
                address_mode(texture.boundary_v),
            );
            load_context.add_labeled_asset(format!("Texture{i}"), image)
        })
        .collect();

    file.meshes
        .iter()
        .enumerate()
        .map(|(i, tri)| {
            let mesh = build_mesh(tri, skin.and_then(|s| s.get(i)));
            let material = build_material(tri, file, &textures);
            ModelPart {
                mesh: load_context.add_labeled_asset(format!("Mesh{i}"), mesh),
                material: load_context.add_labeled_asset(format!("Material{i}"), material),
            }
        })
        .collect()
}

fn build_mesh(tri: &TriMesh, skin: Option<&SkinnedMesh>) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    let positions = skin.map_or_else(|| tri.positions.clone(), |s| s.positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    if let Some(uvs) = &tri.uvs {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone());
    }
    if let Some(colors) = &tri.colors {
        // The original's colours are display values with no gamma handling,
        // which corresponds to sRGB; Bevy's vertex colours are linear.
        let linear: Vec<[f32; 4]> = colors
            .iter()
            .map(|&[r, g, b]| Color::srgb(r, g, b).to_linear().to_f32_array())
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, linear);
    }
    if let Some(skin) = skin {
        let joints: Vec<[u16; 4]> = skin.joints.iter().map(|&j| [j, 0, 0, 0]).collect();
        let weights = vec![[1.0f32, 0.0, 0.0, 0.0]; joints.len()];
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(joints),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
    }
    mesh.insert_indices(Indices::U32(
        tri.triangles.iter().flatten().copied().collect(),
    ));

    let normals = skin
        .map(|s| s.normals.clone())
        .or_else(|| tri.normals.clone());
    match normals {
        Some(normals) => mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals),
        // Pomme computes nothing for meshes without normals either, but Bevy's
        // lit materials need them.
        None => mesh.compute_smooth_normals(),
    }
    mesh
}

/// Port of the per-mesh material set-up in `Render_SubmitMesh`
/// (original/src/QD3D/Renderer.c): diffuse colour times vertex colour times
/// texture, with alpha testing at 0.5 (`glAlphaFunc(GL_GREATER, 0.4999f)`).
fn build_material(tri: &TriMesh, file: &MetaFile, textures: &[Handle<Image>]) -> StandardMaterial {
    let [r, g, b, a] = tri.diffuse_color;
    let texture = tri
        .texture
        .and_then(|t| Some((textures.get(t)?, file.textures.get(t)?)));
    let mut alpha_mode = match texture.map(|(_, t)| t.texturing_mode()) {
        Some(TexturingMode::AlphaTest) => AlphaMode::Mask(0.5),
        Some(TexturingMode::AlphaBlend) => AlphaMode::Blend,
        Some(TexturingMode::Opaque) | None => AlphaMode::Opaque,
    };
    if a < 1.0 {
        alpha_mode = AlphaMode::Blend;
    }
    StandardMaterial {
        base_color: Color::srgba(r, g, b, a),
        base_color_texture: texture.map(|(handle, _)| handle.clone()),
        alpha_mode,
        // The original uses fixed-function Gouraud lighting with no specular
        // highlights; a fully rough, non-reflective surface is the closest match.
        perceptual_roughness: 1.0,
        metallic: 0.0,
        reflectance: 0.0,
        ..default()
    }
}
