//! Binds a skeleton to its geometry: works out which bone moves each vertex of
//! the skeleton's `.3dmf`, and the vertex's position relative to that bone.
//!
//! Port of `LoadBonesReferenceModel` and `DecomposeATriMesh`
//! (original/src/Skeleton/Bones.c), restated for GPU skinning.
//!
//! The original skins on the CPU: every frame, `UpdateSkinnedGeometry` walks
//! the bones depth-first from bone 0 and writes `bone_matrix * relative_point`
//! into every vertex that shares the bone's decomposed points. Each vertex
//! therefore follows exactly one bone (rigid skinning). We express that as
//! one joint per vertex with weight 1, a bind pose in which each bone sits at
//! its absolute [`Bone::coord`](crate::skeleton::Bone::coord) without
//! rotation, and vertex positions `coord + relative_point`. Skinning then
//! computes `joint_matrix * translate(-coord) * (coord + relative_point)`,
//! which is exactly the original's `bone_matrix * relative_point`.
//!
//! Intentional difference: the original rotates each vertex normal by
//! whichever bone last transformed the *merged normal* the vertex refers to.
//! Normals are merged by direction alone, so two body parts that share a
//! normal direction can swap normals. Here each normal follows the same bone
//! as its vertex, which is what the original intends.

use crate::error::{Error, Result};
use crate::skeleton::SkeletonDefinition;
use crate::tdmf::{MetaFile, TriMesh};

/// Per-axis tolerance under which two points are the same decomposed point
/// (`PointsAreCloseEnough` in original/src/QD3D/3DMath.c).
const POINT_TOLERANCE: f32 = 0.001;

/// One mesh of a skinned model, with the skinning attributes added.
#[derive(Debug, Clone, PartialEq)]
pub struct SkinnedMesh {
    /// Bind-pose positions: the owning bone's coord plus the point's
    /// relative offset. Same order as the source mesh's points.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals in the bind pose.
    pub normals: Vec<[f32; 3]>,
    /// The bone that moves each vertex.
    pub joints: Vec<u16>,
}

/// Binds `skeleton` to `model`, the skeleton's `.3dmf`. Returns one
/// [`SkinnedMesh`] per entry of `model.meshes`, in the same order. Triangles,
/// UVs, colours and materials are unchanged, so take them from `model`.
pub fn bind(skeleton: &SkeletonDefinition, model: &MetaFile) -> Result<Vec<SkinnedMesh>> {
    let decomposed = decompose_points(&model.meshes);
    let point_count = decomposed.points.len();
    if point_count != skeleton.relative_points.len() {
        return Err(Error::invalid(format!(
            "the model has {point_count} distinct points but the skeleton has {} relative points",
            skeleton.relative_points.len()
        )));
    }

    let owners = point_owners(skeleton, point_count)?;

    model
        .meshes
        .iter()
        .zip(&decomposed.vertex_points)
        .enumerate()
        .map(|(mesh_index, (mesh, vertex_points))| {
            let normals = mesh.normals.as_ref().ok_or_else(|| {
                Error::invalid(format!("skeleton mesh {mesh_index} has no vertex normals"))
            })?;
            let mut out = SkinnedMesh {
                positions: Vec::with_capacity(vertex_points.len()),
                normals: normals.iter().map(|&n| normalize(n)).collect(),
                joints: Vec::with_capacity(vertex_points.len()),
            };
            for &point in vertex_points {
                let bone = owners[point];
                let coord = skeleton.bones[bone].coord;
                let rel = skeleton.relative_points[point];
                out.positions
                    .push([coord[0] + rel[0], coord[1] + rel[1], coord[2] + rel[2]]);
                // Bone counts are checked against `MAX_JOINTS` (20) by the parser.
                out.joints.push(bone as u16);
            }
            Ok(out)
        })
        .collect()
}

struct Decomposed {
    /// The distinct points, in order of first appearance.
    points: Vec<[f32; 3]>,
    /// For each mesh, for each vertex, its index into `points`.
    vertex_points: Vec<Vec<usize>>,
}

/// Merges points that are within [`POINT_TOLERANCE`] of an earlier one, in the
/// same order as `DecomposeATriMesh`, so indices match the skeleton file.
fn decompose_points(meshes: &[TriMesh]) -> Decomposed {
    let mut points: Vec<[f32; 3]> = Vec::new();
    let vertex_points = meshes
        .iter()
        .map(|mesh| {
            mesh.positions
                .iter()
                .map(|&p| {
                    // First match wins, like the original's linear search.
                    match points.iter().position(|&q| close_enough(p, q)) {
                        Some(i) => i,
                        None => {
                            points.push(p);
                            points.len() - 1
                        }
                    }
                })
                .collect()
        })
        .collect();
    Decomposed {
        points,
        vertex_points,
    }
}

fn close_enough(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < POINT_TOLERANCE)
}

/// The bone that moves each decomposed point. Where several bones list the
/// same point, the original's depth-first walk writes it once per bone, so
/// the last bone visited wins.
fn point_owners(skeleton: &SkeletonDefinition, point_count: usize) -> Result<Vec<usize>> {
    let mut owners = vec![None; point_count];
    let mut stack = vec![0];
    while let Some(bone) = stack.pop() {
        for &point in &skeleton.bones[bone].point_indices {
            let slot = owners.get_mut(usize::from(point)).ok_or_else(|| {
                Error::invalid(format!(
                    "bone {bone} lists point {point}, but there are only {point_count}"
                ))
            })?;
            *slot = Some(bone);
        }
        // Push in reverse so children are visited in `children` order.
        let children: Vec<_> = skeleton.children(bone).collect();
        stack.extend(children.into_iter().rev());
    }
    owners
        .into_iter()
        .enumerate()
        .map(|(point, owner)| {
            owner.ok_or_else(|| Error::invalid(format!("point {point} has no bone")))
        })
        .collect()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{original_data_dir, skeleton, tdmf};

    fn skeleton_names() -> Vec<String> {
        let mut names: Vec<_> = std::fs::read_dir(original_data_dir().join("Skeletons"))
            .unwrap()
            .filter_map(|e| {
                let name = e.unwrap().file_name().into_string().unwrap();
                name.strip_suffix(".skeleton.rsrc").map(str::to_owned)
            })
            .collect();
        names.sort();
        names
    }

    fn load(name: &str) -> (SkeletonDefinition, MetaFile) {
        let dir = original_data_dir().join("Skeletons");
        let skeleton = skeleton::open(dir.join(format!("{name}.skeleton.rsrc"))).unwrap();
        let model = tdmf::open(dir.join(format!("{name}.3dmf"))).unwrap();
        (skeleton, model)
    }

    #[test]
    fn binds_every_skeleton() {
        let names = skeleton_names();
        assert_eq!(names.len(), 24);
        for name in names {
            let (skeleton, model) = load(&name);
            let meshes = bind(&skeleton, &model).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(meshes.len(), model.meshes.len());
            for (skinned, mesh) in meshes.iter().zip(&model.meshes) {
                assert_eq!(skinned.positions.len(), mesh.positions.len());
                assert_eq!(skinned.normals.len(), mesh.positions.len());
                assert!(
                    skinned
                        .joints
                        .iter()
                        .all(|&j| usize::from(j) < skeleton.bones.len())
                );
            }
        }
    }

    /// The bind pose should reproduce the modelled geometry, which confirms
    /// that the decomposition matches the skeleton files: each relative
    /// point is the modelled point minus its bone's coordinate. Points shared
    /// by several bones are skipped, since their offset is only right for one.
    #[test]
    fn bind_pose_matches_modelled_geometry() {
        for name in skeleton_names() {
            let (skeleton, model) = load(&name);
            let meshes = bind(&skeleton, &model).unwrap();
            let mut listed = vec![0u32; skeleton.relative_points.len()];
            for bone in &skeleton.bones {
                for &p in &bone.point_indices {
                    listed[usize::from(p)] += 1;
                }
            }
            let decomposed = decompose_points(&model.meshes);
            let mut worst = 0.0f32;
            for (m, (skinned, mesh)) in meshes.iter().zip(&model.meshes).enumerate() {
                for (v, (a, b)) in skinned.positions.iter().zip(&mesh.positions).enumerate() {
                    if listed[decomposed.vertex_points[m][v]] != 1 {
                        continue;
                    }
                    for i in 0..3 {
                        worst = worst.max((a[i] - b[i]).abs());
                    }
                }
            }
            // DoodleBug's model was scaled up by about 2.7% after it was
            // rigged. The game draws the rig's relative points, as we do.
            let tolerance = if name == "DoodleBug" { 4.0 } else { 0.01 };
            assert!(worst < tolerance, "{name}: bind pose is off by {worst}");
        }
    }
}
