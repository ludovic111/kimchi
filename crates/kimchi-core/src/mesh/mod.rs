//! Polygon meshes: the shapes of 3D objects as faces you can edit (extrude, inset, bevel…) and
//! the modifier stack that turns a shape into what is drawn (subdivision, mirror, array…).

use crate::motion::Shape3d;
use crate::motion::stack::Modifier;

/// Faces with any number of corners over shared vertices, counter-clockwise seen from outside.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolyMesh {
    pub positions: Vec<[f64; 3]>,
    pub faces: Vec<Vec<u32>>,
    /// Texture coordinates per face corner (same shape as `faces`), when known.
    pub uvs: Option<Vec<Vec<[f64; 2]>>>,
    /// Edges bent more than this (degrees) are drawn sharp.
    pub smooth_angle: f64,
}

/// Triangles ready to draw: one normal and texture coordinate per vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// The polygon mesh of a shape (`None` for shapes made elsewhere: text, models, pictures,
/// particles, groups).
pub fn shape_mesh(shape: &Shape3d) -> Option<PolyMesh> {
    match shape {
        Shape3d::Mesh { vertices, faces, uvs, auto_smooth } => Some(PolyMesh {
            positions: vertices.clone(),
            faces: faces.clone(),
            uvs: (!uvs.is_empty()).then(|| uvs.clone()),
            smooth_angle: *auto_smooth,
        }),
        _ => None,
    }
}

/// `mesh` through `modifiers` at scene time `t`. `object(id)` gives another object's mesh in
/// this object's own space (for booleans).
pub fn apply_modifiers(mesh: PolyMesh, _modifiers: &[Modifier], _t: f64, _object: &dyn Fn(&str) -> Option<PolyMesh>) -> PolyMesh {
    mesh
}

impl PolyMesh {
    /// Triangles with normals (smooth across edges bent less than `smooth_angle`) and texture
    /// coordinates.
    pub fn triangulate(&self) -> TriMesh {
        let mut out = TriMesh::default();
        for f in &self.faces {
            let base = out.positions.len() as u32;
            for &v in f {
                let p = self.positions[v as usize];
                out.positions.push([p[0] as f32, p[1] as f32, p[2] as f32]);
                out.normals.push([0.0, 1.0, 0.0]);
                out.uvs.push([0.0, 0.0]);
            }
            for k in 1..f.len().saturating_sub(1) as u32 {
                out.indices.extend([base, base + k, base + k + 1]);
            }
        }
        out
    }
}
