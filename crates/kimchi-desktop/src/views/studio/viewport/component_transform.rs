//! Mesh component G/R/S and gizmos use the same transform math as scene objects. Each preview
//! starts from the original positions and writes only the selected vertices through the registry.
use super::*;
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct MeshTarget {
    clip: Id,
    id: String,
    indices: Vec<u32>,
    vertex_count: usize,
    active_pivot: Option<V3>,
}

impl MeshTarget {
    pub(super) fn commands(&self, session:&Session, key:&str, cancel:bool)->Vec<(String,Value)> {
        if (!cancel && !session.valid) || session.reached.is_empty() {return vec![];}
        if !cancel && session.reached.len()!=self.indices.len() {return vec![];}
        let vertices:Vec<_>=self.indices.iter().enumerate().map(|(i,index)| {
            let position=if cancel {session.starts[i].position} else {session.reached[i].1};
            json!({"index":index,"position":position})
        }).collect();
        vec![("motion.updateMeshVertices".into(),json!({"clipId":self.clip,"id":self.id,"vertices":vertices,
            "expectedVertexCount":self.vertex_count,"coalesce":key}))]
    }
}

impl Viewport {
    pub(super) fn transform_starts(&self, scene:&Scene3d, full_scene:&Scene, time:f64, cx:&App)->Option<(Vec<gizmo::Start>,Option<MeshTarget>)> {
        let studio=self.studio.read(cx);
        let worlds=studio.worlds(scene,time,cx);
        if studio.mode==Mode::Object {
            return Some((gizmo::starts(scene,full_scene,&studio.selection,time,&worlds),None));
        }
        if studio.mesh_busy {return None;}
        let id=studio.active()?;
        let object=kimchi_core::motion::find_object(&scene.objects,id)?;
        let kimchi_core::motion::Shape3d::Mesh {vertices,faces,..}=&object.shape else {return None};
        let selection=studio.mesh_selection(cx);
        let indices=selection.all_vertices(faces);
        if indices.is_empty() {return None;}
        let world=worlds.get(id).copied().unwrap_or(math::IDENTITY);
        let linear=std::array::from_fn(|r|std::array::from_fn(|c|world[c][r]));
        math::m3_inverse(&linear).filter(|inverse|inverse.iter().flatten().all(|v|v.is_finite()))?;
        let position=|index:u32| vertices.get(index as usize).map(|p|math::point(&world,*p));
        let mean=|indices:&[u32]| {
            let points:Vec<_>=indices.iter().filter_map(|i|position(*i)).collect();
            (!points.is_empty()).then(||math::scale(points.iter().copied().fold([0.;3],math::add),1./points.len() as f64))
        };
        let active_pivot=match studio.select_mode {
            SelectMode::Vertex=>studio.edit_sel.vertices.last().and_then(|i|position(*i)),
            SelectMode::Edge=>studio.edit_sel.edges.last().and_then(|(a,b)|mean(&[*a,*b])),
            SelectMode::Face=>studio.edit_sel.faces.last().and_then(|i|faces.get(*i as usize)).and_then(|f|mean(f)),
        };
        let islands=if studio.pivot==gizmo::Pivot::Individual {island_pivots(&selection,faces,&indices,&position)} else {HashMap::new()};
        let starts:Vec<_>=indices.iter().map(|&index|gizmo::Start::mesh_vertex(index,vertices[index as usize],world,islands.get(&index).copied())).collect();
        if starts.iter().any(|s|math::origin(&s.world).iter().any(|n|!n.is_finite())) {return None;}
        let target=MeshTarget {clip:studio.clip?,id:id.into(),indices,vertex_count:vertices.len(),active_pivot};
        Some((starts,Some(target)))
    }

    pub(super) fn transform_frame(&self, starts:&[gizmo::Start], target:Option<&MeshTarget>, cx:&App)->Option<gizmo::Frame> {
        let studio=self.studio.read(cx);
        let mut frame=gizmo::frame(starts,studio.local,studio.pivot)?;
        if studio.pivot==gizmo::Pivot::Active && let Some(active)=target.and_then(|t|t.active_pivot) {frame.pivot=active;}
        frame.pivot.iter().all(|p|p.is_finite()).then_some(frame)
    }
}

/// Connected selected components share a pivot. Explicit edges do not join across unselected
/// edges, even when their endpoints would imply a complete face.
fn island_pivots(selection:&super::super::EditSel, faces:&[Vec<u32>], indices:&[u32], position:&impl Fn(u32)->Option<V3>)->HashMap<u32,V3> {
    let selected:HashSet<_>=indices.iter().copied().collect();
    let edges=if !selection.edges.is_empty() {selection.edges.clone()}
        else if !selection.faces.is_empty() {model::mesh_edges(&selection.faces.iter().filter_map(|i|faces.get(*i as usize).cloned()).collect::<Vec<_>>())}
        else {model::mesh_edges(faces)};
    let mut neighbours:HashMap<u32,Vec<u32>>=HashMap::new();
    for (a,b) in edges {
        if selected.contains(&a) && selected.contains(&b) {neighbours.entry(a).or_default().push(b);neighbours.entry(b).or_default().push(a);}
    }
    let mut unseen=selected;let mut out=HashMap::new();
    while let Some(&first)=unseen.iter().next() {
        unseen.remove(&first);let mut island=vec![first];let mut next=0;
        while next<island.len() {
            if let Some(adjacent)=neighbours.get(&island[next]) {for index in adjacent {if unseen.remove(index) {island.push(*index);}}}
            next+=1;
        }
        let pivot=math::scale(island.iter().filter_map(|i|position(*i)).fold([0.;3],math::add),1./island.len() as f64);
        for index in island {out.insert(index,pivot);}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn individual_mesh_pivots_keep_explicit_edge_islands_separate() {
        let points=[[0.,0.,0.],[2.,0.,0.],[2.,2.,0.],[0.,2.,0.]];
        let selection=super::super::super::EditSel {edges:vec![(0,1),(2,3)],..Default::default()};
        let pivots=island_pivots(&selection,&[vec![0,1,2,3]],&[0,1,2,3],&|i|Some(points[i as usize]));
        assert_eq!(pivots[&0],[1.,0.,0.]);assert_eq!(pivots[&1],[1.,0.,0.]);
        assert_eq!(pivots[&2],[1.,2.,0.]);assert_eq!(pivots[&3],[1.,2.,0.]);
        let starts:Vec<_>=points.into_iter().enumerate().map(|(i,p)|gizmo::Start::mesh_vertex(i as u32,p,math::IDENTITY,Some(pivots[&(i as u32)]))).collect();
        let frame=gizmo::frame(&starts,false,gizmo::Pivot::Individual).unwrap();
        let view=View3 {cam:ViewCamera::default(),x:0.,y:0.,w:800.,h:600.};
        let mut session=Session::new(Kind::Scale,Handle::Free,frame,starts,[0.,0.],&view);
        session.typed="2".into();session.update(&view,[0.,0.]);
        assert_eq!(session.reached.iter().map(|s|s.1).collect::<Vec<_>>(),vec![[-1.,0.,0.],[3.,0.,0.],[3.,2.,0.],[-1.,2.,0.]]);
    }
}
