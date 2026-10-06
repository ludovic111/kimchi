//! Mesh component selection, independent of the viewport camera and renderer.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use gpui::{App, Context};
use kimchi_core::motion::{Shape3d, find_object};
use kimchi_core::{Id, Project, Scene};

use super::{EditSel, KeyRef, Mode, SelectMode, Studio};

/// How a box gesture or external client combines a set with the current selection.
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub enum SelectionOp {
    #[default]
    Replace,
    Add,
    Subtract,
}

impl SelectionOp {
    pub fn parse(value:&str)->Result<Self,String> {
        match value {
            "replace"=>Ok(Self::Replace),"add"=>Ok(Self::Add),"subtract"=>Ok(Self::Subtract),
            _=>Err("selectionOp must be replace, add or subtract".into()),
        }
    }

    pub fn apply<T:Clone+Eq+std::hash::Hash>(self,current:&[T],incoming:Vec<T>)->Vec<T> {
        self.apply_by(current,incoming,Clone::clone)
    }

    pub fn apply_keys(self,current:&[KeyRef],incoming:Vec<KeyRef>)->Vec<KeyRef> {
        self.apply_by(current,incoming,|k| (k.id.clone(),k.property.clone(),if k.time==0. {0} else {k.time.to_bits()}))
    }

    pub fn from_modifiers(modifiers:gpui::Modifiers)->Self {
        let ctrl=modifiers.control || modifiers.platform;
        if modifiers.shift && ctrl {Self::Subtract} else if modifiers.shift || ctrl {Self::Add} else {Self::Replace}
    }

    fn apply_by<T:Clone,K:Eq+std::hash::Hash>(self,current:&[T],incoming:Vec<T>,key:impl Fn(&T)->K)->Vec<T> {
        match self {
            Self::Replace=>{
                let mut seen=HashSet::new();let mut result=incoming;
                result.reverse();result.retain(|v|seen.insert(key(v)));result.reverse();result
            }
            Self::Add=>{
                let mut result=current.to_vec();let mut seen:HashSet<_>=current.iter().map(&key).collect();
                result.extend(incoming.into_iter().filter(|v|seen.insert(key(v))));result
            }
            Self::Subtract=>{
                let removed:HashSet<_>=incoming.iter().map(&key).collect();
                current.iter().filter(|v|!removed.contains(&key(v))).cloned().collect()
            }
        }
    }
}

pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("all", "All", "Select every component in the current selection mode"),
    ("none", "None", "Clear the mesh selection"),
    ("invert", "Invert", "Select the components that are not selected"),
    ("linked", "Linked", "Select connected geometry from the current selection"),
    ("grow", "Grow", "Extend the selection by one neighbouring component"),
    ("shrink", "Shrink", "Remove the outermost selected components next to unselected geometry"),
    ("boundary", "Boundary", "Select open borders of the mesh"),
];

pub const EDGE_ACTIONS: &[(&str,&str,&str)] = &[
    ("edgeLoop","Edge loop","Extend selected edges along loops; stops at irregular vertices. Edge mode only."),
    ("edgeRing","Edge ring","Extend selected edges across opposite sides of quads. Edge mode only."),
];

pub struct Topology {
    pub vertices: usize,
    pub faces: Vec<Vec<u32>>,
    pub edges: Vec<(u32, u32)>,
    vertex_edges: Vec<Vec<usize>>,
    edge_faces: Vec<Vec<usize>>,
}

impl Topology {
    pub fn new(vertices: usize, faces: Vec<Vec<u32>>) -> Self {
        let mut edges = super::model::mesh_edges(&faces);
        edges.sort_unstable();
        let index: HashMap<_, _> = edges.iter().enumerate().map(|(i, &e)| (e, i)).collect();
        let mut vertex_edges = vec![vec![]; vertices];
        let mut edge_faces = vec![vec![]; edges.len()];
        for (i, &(a, b)) in edges.iter().enumerate() {
            vertex_edges[a as usize].push(i);
            vertex_edges[b as usize].push(i);
        }
        for (fi, f) in faces.iter().enumerate() {
            for (&a, &b) in f.iter().zip(f.iter().cycle().skip(1)).take(f.len()) {
                if let Some(&i) = index.get(&(a.min(b), a.max(b))) {
                    edge_faces[i].push(fi);
                }
            }
        }
        Self { vertices, faces, edges, vertex_edges, edge_faces }
    }

    /// A selection may outlive undo, replacement or an edit from another client.
    pub fn sanitize(&self, s: &mut EditSel) {
        s.vertices.retain(|&v| (v as usize) < self.vertices);
        s.faces.retain(|&f| (f as usize) < self.faces.len());
        let edges: HashSet<_> = self.edges.iter().copied().collect();
        for e in &mut s.edges {
            *e = (e.0.min(e.1), e.0.max(e.1));
        }
        s.edges.retain(|e| edges.contains(e));
        // The last picked component is the active pivot. Pruning stale indices must not
        // reorder the selection when an edit, undo or another client refreshes the scene.
        fn unique<T: Copy + Eq + std::hash::Hash>(items: &mut Vec<T>) {
            let mut seen = HashSet::new();
            items.reverse();
            items.retain(|v| seen.insert(*v));
            items.reverse();
        }
        unique(&mut s.vertices);
        unique(&mut s.faces);
        unique(&mut s.edges);
    }

    pub fn validate(&self, s: &mut EditSel) -> Result<(), String> {
        if let Some(v) = s.vertices.iter().find(|&&v| v as usize >= self.vertices) {
            return Err(format!("Vertex {v} is outside this mesh ({} vertices).", self.vertices));
        }
        if let Some(f) = s.faces.iter().find(|&&f| f as usize >= self.faces.len()) {
            return Err(format!("Face {f} is outside this mesh ({} faces).", self.faces.len()));
        }
        let edges: HashSet<_> = self.edges.iter().copied().collect();
        if let Some(&(a, b)) = s.edges.iter().find(|&&(a, b)| !edges.contains(&(a.min(b), a.max(b)))) {
            return Err(format!("Vertices {a} and {b} do not share an edge in this mesh."));
        }
        self.sanitize(s);
        Ok(())
    }

    /// Converting a mode keeps full components only: an edge becomes a face only when
    /// every edge of that face was selected, not merely when its endpoints were touched.
    pub fn convert(&self, s: &EditSel, mode: SelectMode) -> EditSel {
        let vs: HashSet<_> = s.vertices.iter().copied().collect();
        let fs: HashSet<_> = s.faces.iter().copied().collect();
        let es: HashSet<_> = s.edges.iter().map(|&(a, b)| (a.min(b), a.max(b))).collect();
        match mode {
            SelectMode::Vertex => EditSel { vertices: s.all_vertices(&self.faces), ..Default::default() },
            SelectMode::Edge => EditSel {
                edges: self
                    .edges
                    .iter()
                    .enumerate()
                    .filter(|(i, (a, b))| {
                        es.contains(&(*a, *b)) || (vs.contains(a) && vs.contains(b)) || self.edge_faces[*i].iter().any(|f| fs.contains(&(*f as u32)))
                    })
                    .map(|(_, &e)| e)
                    .collect(),
                ..Default::default()
            },
            SelectMode::Face => EditSel {
                faces: self
                    .faces
                    .iter()
                    .enumerate()
                    .filter(|(i, f)| {
                        fs.contains(&(*i as u32))
                            || f.iter().all(|v| vs.contains(v))
                            || f.iter().zip(f.iter().cycle().skip(1)).take(f.len()).all(|(&a, &b)| es.contains(&(a.min(b), a.max(b))))
                    })
                    .map(|(i, _)| i as u32)
                    .collect(),
                ..Default::default()
            },
        }
    }

    pub fn apply(&self, s: &EditSel, mode: SelectMode, action: &str) -> Result<EditSel, String> {
        if !ACTIONS.iter().any(|(a, _, _)| *a == action) {
            return Err(format!("Unknown meshSelect {action:?}; use all, none, invert, linked, grow, shrink or boundary."));
        }
        let s = self.convert(s, mode);
        let mut selected: Vec<bool> = match mode {
            SelectMode::Vertex => {
                let set: HashSet<_> = s.vertices.iter().copied().collect();
                (0..self.vertices).map(|i| set.contains(&(i as u32))).collect()
            }
            SelectMode::Edge => {
                let set: HashSet<_> = s.edges.iter().copied().collect();
                self.edges.iter().map(|e| set.contains(e)).collect()
            }
            SelectMode::Face => {
                let set: HashSet<_> = s.faces.iter().copied().collect();
                (0..self.faces.len()).map(|i| set.contains(&(i as u32))).collect()
            }
        };
        match action {
            "all" => selected.fill(true),
            "none" => selected.fill(false),
            "invert" => selected.iter_mut().for_each(|v| *v = !*v),
            "boundary" => {
                selected.fill(false);
                for (i, fs) in self.edge_faces.iter().enumerate().filter(|(_, fs)| fs.len() == 1) {
                    match mode {
                        SelectMode::Vertex => {
                            let (a, b) = self.edges[i];
                            selected[a as usize] = true;
                            selected[b as usize] = true;
                        }
                        SelectMode::Edge => selected[i] = true,
                        SelectMode::Face => selected[fs[0]] = true,
                    }
                }
            }
            _ => {
                // Incident groups avoid a quadratic adjacency list at high-valence vertices.
                let groups = match mode {
                    SelectMode::Vertex => self.edges.iter().map(|&(a, b)| vec![a as usize, b as usize]).collect::<Vec<_>>(),
                    SelectMode::Edge => self.vertex_edges.clone(),
                    SelectMode::Face => self.edge_faces.clone(),
                };
                if action == "linked" {
                    let mut membership = vec![vec![]; selected.len()];
                    for (g, items) in groups.iter().enumerate() {
                        for &i in items {
                            membership[i].push(g);
                        }
                    }
                    let mut seen = vec![false; groups.len()];
                    let mut queue: VecDeque<_> = selected.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
                    while let Some(i) = queue.pop_front() {
                        for &g in &membership[i] {
                            if std::mem::replace(&mut seen[g], true) {
                                continue;
                            }
                            for &j in &groups[g] {
                                if !selected[j] {
                                    selected[j] = true;
                                    queue.push_back(j);
                                }
                            }
                        }
                    }
                } else {
                    let before = selected.clone();
                    for group in groups {
                        if action == "grow" && group.iter().any(|&i| before[i]) {
                            for i in group {
                                selected[i] = true;
                            }
                        } else if action == "shrink" && group.iter().any(|&i| !before[i]) {
                            for i in group {
                                selected[i] = false;
                            }
                        }
                    }
                }
            }
        }
        let ids = || selected.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i);
        Ok(match mode {
            SelectMode::Vertex => EditSel { vertices: ids().map(|i| i as u32).collect(), ..Default::default() },
            SelectMode::Edge => EditSel { edges: ids().map(|i| self.edges[i]).collect(), ..Default::default() },
            SelectMode::Face => EditSel { faces: ids().map(|i| i as u32).collect(), ..Default::default() },
        })
    }
}

pub(super) struct CachedTopology {
    project: Arc<Project>,
    clip: Id,
    id: String,
    mesh: Arc<Topology>,
}

impl Studio {
    pub fn mesh_topology(&self, cx: &App) -> Option<Arc<Topology>> {
        let project = self.project(cx)?;
        let clip = self.clip?;
        let id = self.active()?;
        let mut cached = self.mesh_topology_cache.borrow_mut();
        if let Some(c) = cached.as_ref()
            && Arc::ptr_eq(&c.project, &project)
            && c.clip == clip
            && c.id == id
        {
            return Some(c.mesh.clone());
        }
        let (_, Scene::Space(scene)) = super::model::motion_clip(&project, clip)? else { return None };
        let object = find_object(&scene.objects, id)?;
        let Shape3d::Mesh { vertices, faces, .. } = &object.shape else { return None };
        let mesh = Arc::new(Topology::new(vertices.len(), faces.clone()));
        *cached = Some(CachedTopology { project, clip, id: id.into(), mesh: mesh.clone() });
        Some(mesh)
    }

    pub fn mesh_selection(&self, cx: &App) -> EditSel {
        self.mesh_topology(cx).map(|m| m.convert(&self.edit_sel, self.select_mode)).unwrap_or_default()
    }

    pub fn select_mesh(&mut self, action: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.mode != Mode::Edit {
            return Err("Enter mesh edit mode first.".into());
        }
        let mesh = self.mesh_topology(cx).ok_or("Select a mesh object first.")?;
        self.edit_sel = if EDGE_ACTIONS.iter().any(|(name,_,_)| *name==action) {
            if self.select_mode!=SelectMode::Edge {return Err("Switch to edge selection mode to select loops or rings.".into());}
            let seeds=mesh.convert(&self.edit_sel,SelectMode::Edge).edges;
            if seeds.is_empty() {return Err("Select at least one edge first.".into());}
            let Some((_,Scene::Space(scene)))=self.clip_scene(cx) else {return Err("Select a mesh object first.".into())};
            let object=find_object(&scene.objects,self.active().unwrap_or("")).ok_or("Select a mesh object first.")?;
            let Shape3d::Mesh {vertices,faces,..}=&object.shape else {return Err("Select a mesh object first.".into())};
            let geometry=kimchi_core::mesh::PolyMesh::new(vertices.clone(),faces.clone());
            let mut edges=HashSet::new();
            for (a,b) in seeds {
                if edges.contains(&(a,b)) {continue;}
                let path=if action=="edgeLoop" {kimchi_core::mesh::ops::edge_loop(&geometry,a,b)}
                    else {kimchi_core::mesh::ops::edge_ring(&geometry,a,b)};
                edges.extend(path.edges);
            }
            let mut edges:Vec<_>=edges.into_iter().collect();
            edges.sort_unstable();
            EditSel {edges,..Default::default()}
        } else {mesh.apply(&self.edit_sel, self.select_mode, action)?};
        self.focus_area(super::Area::Viewport,cx);
        self.changed(cx);
        Ok(())
    }

    pub fn set_select_mode(&mut self, mode: SelectMode, cx: &mut Context<Self>) {
        self.focus_area(super::Area::Viewport,cx);
        if self.select_mode != mode
            && let Some(mesh) = self.mesh_topology(cx)
        {
            self.edit_sel = mesh.convert(&self.edit_sel, mode);
        }
        self.select_mode = mode;
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip() -> Topology {
        // Three adjacent quads and a disconnected triangle; vertex 11 is loose.
        Topology::new(12, vec![vec![0, 1, 5, 4], vec![1, 2, 6, 5], vec![2, 3, 7, 6], vec![8, 9, 10]])
    }

    #[test]
    fn face_growth_is_one_step_and_linked_stays_on_its_island() {
        let mesh = strip();
        let seed = EditSel { faces: vec![0], ..Default::default() };
        let grown = mesh.apply(&seed, SelectMode::Face, "grow").unwrap();
        assert_eq!(grown.faces, [0, 1]);
        assert_eq!(mesh.apply(&grown, SelectMode::Face, "shrink").unwrap().faces, [0]);
        assert_eq!(mesh.apply(&seed, SelectMode::Face, "linked").unwrap().faces, [0, 1, 2]);
        assert_eq!(mesh.apply(&seed, SelectMode::Face, "invert").unwrap().faces, [1, 2, 3]);
        let empty = mesh.apply(&grown, SelectMode::Face, "none").unwrap();
        assert!(mesh.apply(&empty, SelectMode::Face, "linked").unwrap().is_empty());
    }

    #[test]
    fn vertices_and_edges_grow_without_crossing_disconnected_geometry() {
        let mesh = strip();
        let seed = EditSel { vertices: vec![0], ..Default::default() };
        assert_eq!(mesh.apply(&seed, SelectMode::Vertex, "grow").unwrap().vertices, [0, 1, 4]);
        assert_eq!(mesh.apply(&seed, SelectMode::Vertex, "linked").unwrap().vertices, (0..8).collect::<Vec<_>>());
        let edge = EditSel { edges: vec![(0, 1)], ..Default::default() };
        let grown = mesh.apply(&edge, SelectMode::Edge, "grow").unwrap();
        assert_eq!(grown.edges.len(), 4); // original, 0-4, 1-2, 1-5
        let linked = mesh.apply(&edge, SelectMode::Edge, "linked").unwrap();
        assert_eq!(linked.edges.len(), 10);
        assert!(linked.edges.iter().all(|&(a, b)| a < 8 && b < 8));
        let loose = EditSel { vertices: vec![11], ..Default::default() };
        assert_eq!(mesh.apply(&loose, SelectMode::Vertex, "linked").unwrap(), loose);
    }

    #[test]
    fn boundary_excludes_internal_edges_and_mode_conversion_is_precise() {
        let mesh = strip();
        let border = mesh.apply(&EditSel::default(), SelectMode::Edge, "boundary").unwrap();
        assert_eq!(border.edges.len(), 11);
        assert!(!border.edges.contains(&(1, 5)));
        assert!(!border.edges.contains(&(2, 6)));
        let face = EditSel { faces: vec![0], ..Default::default() };
        let edges = mesh.convert(&face, SelectMode::Edge);
        assert_eq!(edges.edges.len(), 4);
        assert_eq!(mesh.convert(&edges, SelectMode::Face), face);
        // Opposite edges touch all four vertices, but do not select the face.
        let partial = EditSel { edges: vec![(0, 1), (4, 5)], ..Default::default() };
        assert!(mesh.convert(&partial, SelectMode::Face).is_empty());
        assert_eq!(mesh.convert(&partial, SelectMode::Vertex).vertices, [0, 1, 4, 5]);
    }

    #[test]
    fn stale_or_invalid_selections_never_escape_mesh_bounds() {
        let mesh = strip();
        let mut invalid = EditSel { vertices: vec![99], faces: vec![99], edges: vec![(0, 10)] };
        assert!(mesh.validate(&mut invalid).is_err());
        mesh.sanitize(&mut invalid);
        assert!(invalid.is_empty());
        let mut reversed = EditSel { edges: vec![(1, 0), (0, 1)], ..Default::default() };
        mesh.validate(&mut reversed).unwrap();
        assert_eq!(reversed.edges, [(0, 1)]);
        assert!(mesh.apply(&reversed, SelectMode::Edge, "typo").is_err());
    }

    #[test]
    fn pruning_preserves_pick_order_and_the_last_valid_active_component() {
        let mesh = strip();
        let mut selection = EditSel {
            vertices: vec![5, 2, 5, 0, 99], faces: vec![2, 1, 0, 99],
            edges: vec![(5, 1), (0, 1), (1, 5), (99, 0), (4, 0)],
        };
        mesh.sanitize(&mut selection);
        assert_eq!(selection.vertices, [2, 5, 0]);
        assert_eq!(selection.faces, [2, 1, 0]);
        assert_eq!(selection.edges, [(0, 1), (1, 5), (0, 4)]);
        let before = selection.clone();
        mesh.validate(&mut selection).unwrap();
        assert_eq!(selection, before);
    }
}
