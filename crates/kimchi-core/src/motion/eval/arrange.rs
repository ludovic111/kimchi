//! Object origins arranged in world space, with the same evaluated hierarchy as the renderer.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectArrangement {
    AlignActive,
    AlignCentre,
    Distribute,
}

/// A proposed local position and its expected world position. Callers apply all edits together
/// and verify the result: an expression or constraint on a parent can depend on a moved object.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginEdit {
    pub id: String,
    /// None for a stationary endpoint or anchor, whose world position must still be verified.
    pub position: Option<[f64; 3]>,
    pub world_position: [f64; 3],
}

impl Scene3d {
    /// Align origins to the last independent selected object's origin or the selection's mean, or space
    /// them evenly between the outermost origins. Other world axes stay unchanged. Selected
    /// descendants move with their selected ancestors instead of receiving a second edit.
    pub fn arranged_origins(&self, ids: &[String], axis: usize, arrangement: ObjectArrangement, t: f64, opts: &EvalOptions) -> Result<Vec<OriginEdit>, String> {
        if axis > 2 || !t.is_finite() { return Err("Arrange requires an X, Y or Z axis and a finite time.".into()); }
        let mut solver = Solver::new(self, t, opts);
        let mut selected = HashSet::new();
        let mut indices = vec![];
        for id in ids {
            let Some(Target::Object(i)) = solver.target(id) else { return Err(format!("\"{id}\" is not a 3D object.")) };
            if selected.insert(i) { indices.push(i); }
        }
        indices.retain(|&i| {
            let mut parent = solver.nodes[i].parent;
            while let Some(p) = parent {
                if selected.contains(&p) { return false; }
                parent = solver.nodes[p].parent;
            }
            true
        });
        let needed = if arrangement == ObjectArrangement::Distribute { 3 } else { 2 };
        if indices.len() < needed { return Err(format!("Select at least {needed} objects outside one another's hierarchy.")); }
        let active = *indices.last().expect("enough objects");
        let active_position = solver.object(active).point([0.; 3]);
        let mut origins: Vec<_> = indices.into_iter().map(|i| (i, solver.object(i).point([0.; 3]))).collect();
        let centre = origins.iter().map(|(_, p)| p[axis]).sum::<f64>() / origins.len() as f64;
        if arrangement == ObjectArrangement::Distribute {
            // Stable sorting preserves selection order when two origins coincide.
            origins.sort_by(|a, b| a.1[axis].total_cmp(&b.1[axis]));
        }
        let (first, last) = (origins[0].1[axis], origins[origins.len() - 1].1[axis]);
        let mut edits = vec![];
        for (rank, (i, position)) in origins.iter().enumerate() {
            let o = solver.nodes[*i].src;
            let mut world_position = *position;
            world_position[axis] = match arrangement {
                ObjectArrangement::AlignActive => active_position[axis],
                ObjectArrangement::AlignCentre => centre,
                ObjectArrangement::Distribute => first + (last - first) * rank as f64 / (origins.len() - 1) as f64,
            };
            if length(sub(world_position, *position)) < 1e-10 {
                edits.push(OriginEdit { id: o.id.clone(), position: None, world_position });
                continue;
            }
            if o.expressions.keys().any(|p| p == "position" || p.starts_with("position.") || matches!(p.as_str(), "x" | "y" | "z")) || o.constraints.iter().any(|c| c.enabled) {
                return Err(format!("\"{}\" has a position expression or enabled constraint; disable it before arranging this object.", o.id));
            }
            let parent = match solver.nodes[*i].parent { Some(p) => solver.object(p), None => M4::I };
            let position = parent.try_inverse_point(world_position).ok_or_else(|| format!("\"{}\" has a parent with zero scale; its world position cannot be edited.", o.id))?;
            if position.iter().chain(&world_position).any(|v| !v.is_finite()) { return Err(format!("Arranging \"{}\" would produce an invalid position.", o.id)); }
            edits.push(OriginEdit { id: o.id.clone(), position: Some(position), world_position });
        }
        Ok(edits)
    }

    /// Whether these edits reached their intended world positions after animation and scene
    /// dependencies were evaluated again. This prevents silently ineffective arrangement.
    pub fn verify_arranged_origins(&self, edits: &[OriginEdit], t: f64, opts: &EvalOptions) -> Result<(), String> {
        let mut solver = Solver::new(self, t, opts);
        for edit in edits {
            let Some(Target::Object(i)) = solver.target(&edit.id) else { return Err(format!("Object \"{}\" is gone.", edit.id)) };
            let reached = solver.object(i).point([0.; 3]);
            if reached.iter().zip(edit.world_position).any(|(a, b)| !a.is_finite() || (a - b).abs() > 1e-7 * b.abs().max(1.)) {
                return Err(format!("\"{}\" is driven by another scene dependency; arranging it would not reach the requested position.", edit.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn arrangement_rejects_singular_parents_and_detects_dependencies_after_an_edit() {
        let Scene::Space(mut scene) = Scene::from_json(&json!({"type":"3d","objects":[
            {"id":"parent","type":"group","scale":[0,1,1],"children":[{"id":"child","type":"box"}]},
            {"id":"anchor","type":"box","position":[4,0,0]}
        ]})).unwrap() else { panic!() };
        let ids = vec!["child".into(), "anchor".into()];
        let opts = EvalOptions::default();
        let e = scene.arranged_origins(&ids, 0, ObjectArrangement::AlignActive, 0., &opts).unwrap_err();
        assert!(e.contains("zero scale"), "{e}");
        scene.objects[0].scale.0 = [1.; 3];
        scene.objects[0].expressions.insert("position.x".into(), "prop(\"anchor\", \"position.x\")".into());
        scene.objects[0].children[0].position.0 = [2.,0.,0.];
        let edits = scene.arranged_origins(&ids, 0, ObjectArrangement::AlignCentre, 0., &opts).unwrap();
        assert!(edits.iter().all(|e| e.world_position == [5.,0.,0.]));
        for edit in &edits {
            let target = if edit.id == "anchor" { &mut scene.objects[1] } else { &mut scene.objects[0].children[0] };
            target.position.0 = edit.position.unwrap();
        }
        assert!(scene.verify_arranged_origins(&edits, 0., &opts).unwrap_err().contains("dependency"));
    }
}
