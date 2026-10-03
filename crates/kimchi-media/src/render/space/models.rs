//! Model files besides glTF: Wavefront OBJ (with the colours of its MTL materials and PNG
//! colour maps) and STL (binary or text), read into the same parts as glTF models.
//!
//! OBJ files are y-up like kimchi's world. STL files come from CAD and 3D printing, which put z
//! up: they are turned so their top faces +y.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::Texture;
use super::math::V3;
use super::mesh::{Mesh, Part};

/// A material from an MTL file.
#[derive(Debug, Clone)]
struct ObjMaterial {
    /// Linear RGB and alpha.
    color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: [f32; 3],
    texture: Option<String>,
}

impl Default for ObjMaterial {
    fn default() -> Self {
        ObjMaterial { color: [0.8, 0.8, 0.8, 1.0], roughness: 0.5, metallic: 0.0, emissive: [0.0; 3], texture: None }
    }
}

/// An OBJ file as parts (one per material used), not yet centred.
pub(crate) fn obj(path: &Path) -> Result<Vec<Part>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    parse_obj(&text, &|name| std::fs::read_to_string(dir.join(name)).ok(), &|name| load_png(&dir.join(name)))
}

fn load_png(path: &Path) -> Option<Arc<Texture>> {
    match tiny_skia::Pixmap::load_png(path) {
        Ok(p) => Some(Arc::new(Texture::of(&p))),
        Err(e) => {
            tracing::warn!("model texture {}: {e} (PNG colour maps only)", path.display());
            None
        }
    }
}

/// Reads OBJ text; `read` gives the text of a file next to it (material libraries) and `picture`
/// a colour map.
pub(crate) fn parse_obj(text: &str, read: &dyn Fn(&str) -> Option<String>, picture: &dyn Fn(&str) -> Option<Arc<Texture>>) -> Result<Vec<Part>, String> {
    let mut pos: Vec<[f32; 3]> = vec![];
    let mut uvs: Vec<[f32; 2]> = vec![];
    let mut normals: Vec<[f32; 3]> = vec![];
    let mut materials: HashMap<String, ObjMaterial> = HashMap::new();
    // Triangles per material, as (position, uv, normal) index triples per corner.
    type Corner = (usize, Option<usize>, Option<usize>);
    let mut groups: Vec<(String, Vec<[Corner; 3]>)> = vec![(String::new(), vec![])];
    let num = |s: Option<&str>| s.and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()).unwrap_or(0.0);
    for (line_no, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut words = line.split_whitespace();
        let Some(tag) = words.next() else { continue };
        match tag {
            "v" => pos.push([num(words.next()), num(words.next()), num(words.next())]),
            "vt" => uvs.push([num(words.next()), 1.0 - num(words.next())]),
            "vn" => normals.push([num(words.next()), num(words.next()), num(words.next())]),
            "f" => {
                // Negative indices count back from the last one read; 1-based otherwise.
                let index = |s: &str, len: usize| -> Option<usize> {
                    let i: i64 = s.parse().ok()?;
                    let i = if i < 0 { len as i64 + i } else { i - 1 };
                    (0..len as i64).contains(&i).then_some(i as usize)
                };
                let mut corners: Vec<Corner> = vec![];
                for w in words {
                    let mut parts = w.split('/');
                    let p = parts.next().and_then(|s| index(s, pos.len())).ok_or_else(|| format!("line {}: bad face vertex `{w}`", line_no + 1))?;
                    let t = parts.next().filter(|s| !s.is_empty()).and_then(|s| index(s, uvs.len()));
                    let n = parts.next().filter(|s| !s.is_empty()).and_then(|s| index(s, normals.len()));
                    corners.push((p, t, n));
                }
                let tris = &mut groups.last_mut().expect("one group").1;
                for k in 1..corners.len().saturating_sub(1) {
                    tris.push([corners[0], corners[k], corners[k + 1]]);
                }
            }
            "usemtl" => {
                let name = words.collect::<Vec<_>>().join(" ");
                groups.push((name, vec![]));
            }
            "mtllib" => {
                for lib in words {
                    match read(lib) {
                        Some(t) => materials.extend(parse_mtl(&t)),
                        None => tracing::warn!("OBJ material library `{lib}` not found"),
                    }
                }
            }
            _ => {}
        }
    }
    let mut textures: HashMap<String, Option<Arc<Texture>>> = HashMap::new();
    let mut parts = vec![];
    for (name, tris) in groups {
        if tris.is_empty() {
            continue;
        }
        let mat = materials.get(&name).cloned().unwrap_or_default();
        let mut m = Mesh::default();
        for t in &tris {
            let p: Vec<V3> = t.iter().map(|c| V3::of(pos[c.0])).collect();
            let face = (p[1] - p[0]).cross(p[2] - p[0]).norm();
            for c in t {
                let n = c.2.map(|i| V3::of(normals[i]).norm()).filter(|n| n.finite() && n.len() > 0.5).unwrap_or(face);
                let uv = c.1.map_or([0.0, 0.0], |i| uvs[i]);
                m.pos.push(pos[c.0]);
                m.normal.push(n.arr());
                m.uv.push(uv);
                m.index.push(m.index.len() as u32);
            }
        }
        let texture = mat.texture.as_ref().and_then(|t| textures.entry(t.clone()).or_insert_with(|| picture(t)).clone());
        parts.push(Part { mesh: Arc::new(m), color: mat.color, metallic: mat.metallic, roughness: mat.roughness, emissive: mat.emissive, texture });
    }
    if parts.is_empty() {
        return Err("the OBJ file has no faces".into());
    }
    Ok(parts)
}

/// Materials of an MTL file by name: diffuse colour (`Kd`), shininess (`Ns`), opacity (`d`,
/// `Tr`), glow (`Ke`), metalness (`Pm`), roughness (`Pr`) and the colour map (`map_Kd`).
fn parse_mtl(text: &str) -> HashMap<String, ObjMaterial> {
    let mut out = HashMap::new();
    let mut current: Option<(String, ObjMaterial)> = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut words = line.split_whitespace();
        let Some(tag) = words.next() else { continue };
        let rest: Vec<&str> = words.collect();
        let f = |i: usize| rest.get(i).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
        let rgb = || [f(0).unwrap_or(0.0), f(1).or(f(0)).unwrap_or(0.0), f(2).or(f(0)).unwrap_or(0.0)];
        if tag == "newmtl" {
            if let Some((n, m)) = current.take() {
                out.insert(n, m);
            }
            current = Some((rest.join(" "), ObjMaterial::default()));
            continue;
        }
        let Some((_, m)) = current.as_mut() else { continue };
        match tag {
            "Kd" => {
                let c = rgb();
                m.color = [super::srgb_f(c[0]), super::srgb_f(c[1]), super::srgb_f(c[2]), m.color[3]];
            }
            "Ke" => m.emissive = rgb().map(super::srgb_f),
            "Ns" => {
                let ns = f(0).unwrap_or(10.0).max(0.0);
                m.roughness = (2.0 / (ns + 2.0)).sqrt().clamp(0.04, 1.0);
            }
            "Pr" => m.roughness = f(0).unwrap_or(0.5).clamp(0.0, 1.0),
            "Pm" => m.metallic = f(0).unwrap_or(0.0).clamp(0.0, 1.0),
            "d" => m.color[3] = f(0).unwrap_or(1.0).clamp(0.0, 1.0),
            "Tr" => m.color[3] = 1.0 - f(0).unwrap_or(0.0).clamp(0.0, 1.0),
            // The file name is last (options like `-s 1 1 1` may come first).
            "map_Kd" => m.texture = rest.last().map(|s| s.to_string()),
            _ => {}
        }
    }
    if let Some((n, m)) = current {
        out.insert(n, m);
    }
    out
}

/// An STL file (binary or text) as one part, z-up turned to y-up, flat-shaded.
pub(crate) fn stl(path: &Path) -> Result<Vec<Part>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_stl(&bytes)
}

pub(crate) fn parse_stl(bytes: &[u8]) -> Result<Vec<Part>, String> {
    let mut tris: Vec<[[f32; 3]; 3]> = vec![];
    let binary_len = (bytes.len() >= 84).then(|| 84 + 50 * u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize);
    let looks_text = bytes.starts_with(b"solid") && bytes.windows(5).take(4096).any(|w| w.eq_ignore_ascii_case(b"facet"));
    // Binary files may start with "solid" too: their length is what tells them apart.
    if binary_len == Some(bytes.len()) {
        let n = (bytes.len() - 84) / 50;
        for i in 0..n {
            let at = 84 + i * 50 + 12;
            let f = |k: usize| f32::from_le_bytes([bytes[at + k * 4], bytes[at + k * 4 + 1], bytes[at + k * 4 + 2], bytes[at + k * 4 + 3]]);
            tris.push([[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]]);
        }
    } else if looks_text || bytes.starts_with(b"solid") {
        let text = String::from_utf8_lossy(bytes);
        let mut corner: Vec<[f32; 3]> = vec![];
        for line in text.lines() {
            let mut w = line.split_whitespace();
            if w.next() != Some("vertex") {
                continue;
            }
            let v: Vec<f32> = w.filter_map(|s| s.parse().ok()).collect();
            if v.len() < 3 {
                return Err(format!("bad STL vertex line `{}`", line.trim()));
            }
            corner.push([v[0], v[1], v[2]]);
            if corner.len() == 3 {
                tris.push([corner[0], corner[1], corner[2]]);
                corner.clear();
            }
        }
    } else {
        return Err("not an STL file (neither binary nor text)".into());
    }
    let tris: Vec<_> = tris.into_iter().filter(|t| t.iter().flatten().all(|v| v.is_finite())).collect();
    if tris.is_empty() {
        return Err("the STL file has no triangles".into());
    }
    let mut m = Mesh::default();
    for t in tris {
        // z up → y up: (x, y, z) → (x, z, −y).
        let p: Vec<V3> = t.iter().map(|v| V3(v[0], v[2], -v[1])).collect();
        let n = (p[1] - p[0]).cross(p[2] - p[0]).norm();
        for q in p {
            m.pos.push(q.arr());
            m.normal.push(n.arr());
            m.uv.push([0.0, 0.0]);
            m.index.push(m.index.len() as u32);
        }
    }
    Ok(vec![Part { mesh: Arc::new(m), color: [0.8, 0.8, 0.8, 1.0], metallic: 0.0, roughness: 0.5, emissive: [0.0; 3], texture: None }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_faces_materials_and_negative_indices() {
        let obj = "mtllib box.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nvt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\nvn 0 0 1\n\
                   usemtl red\nf 1/1/1 2/2/1 3/3/1 4/4/1\nusemtl plain\nf -4 -3 -2\n";
        let mtl = "newmtl red\nKd 1 0 0\nNs 200\nd 0.5\nnewmtl plain\nKd 0.5 0.5 0.5\n";
        let parts = parse_obj(obj, &|name| (name == "box.mtl").then(|| mtl.to_string()), &|_| None).unwrap();
        assert_eq!(parts.len(), 2);
        let red = &parts[0];
        assert_eq!(red.mesh.index.len(), 6, "a quad becomes two triangles");
        assert_eq!(red.color, [1.0, 0.0, 0.0, 0.5]);
        assert!(red.roughness < 0.2, "shiny: {}", red.roughness);
        assert_eq!(red.mesh.normal[0], [0.0, 0.0, 1.0]);
        assert_eq!(red.mesh.uv[2], [1.0, 0.0], "v flipped (OBJ's v goes up)");
        let plain = &parts[1];
        assert_eq!(plain.mesh.index.len(), 3);
        // Without normals in the file, the face's own normal.
        assert!((V3::of(plain.mesh.normal[0]) - V3(0.0, 0.0, 1.0)).len() < 1e-6);
        assert!(parse_obj("v 0 0 0\nf 1 2 3\n", &|_| None, &|_| None).is_err(), "indices out of range are an error");
    }

    #[test]
    fn stl_binary_and_text() {
        let text = "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\n\
                    facet normal 0 0 1\nouter loop\nvertex 0 0 1\nvertex 1 0 1\nvertex 0 1 1\nendloop\nendfacet\nendsolid t\n";
        let parts = parse_stl(text.as_bytes()).unwrap();
        assert_eq!(parts[0].mesh.index.len(), 6);
        // z up became y up: the second triangle (z = 1) is now at y = 1.
        assert!(parts[0].mesh.pos[3..].iter().all(|p| (p[1] - 1.0).abs() < 1e-6), "{:?}", parts[0].mesh.pos);
        let mut bin = vec![0u8; 80];
        bin.extend(1u32.to_le_bytes());
        for v in [0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 2.0] {
            bin.extend(v.to_le_bytes());
        }
        bin.extend([0, 0]);
        let parts = parse_stl(&bin).unwrap();
        assert_eq!(parts[0].mesh.pos.len(), 3);
        assert_eq!(parts[0].mesh.pos[1], [2.0, 0.0, -0.0]);
        assert!(parse_stl(b"hello").is_err());
    }
}
