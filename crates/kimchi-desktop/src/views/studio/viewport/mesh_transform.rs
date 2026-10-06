//! Modal mesh tools serialize their geometry edits and finish only after the last result.
use super::*;

impl Viewport {
    /// E extrudes in place and then pulls the new faces; inset and bevel apply on confirmation.
    pub(super) fn start_mesh_modal(&mut self, kind: ModalKind, cx: &mut Context<Self>) {
        let Some((verts, faces, view)) = self.mesh(cx) else { return };
        let st = self.studio.read(cx);
        if st.mesh_busy { return; }
        let Some(clip) = st.clip else { return };
        let Some(id) = st.active().map(str::to_string) else { return };
        let selection = st.mesh_selection(cx);
        if selection.is_empty() {
            super::super::flash("Select some faces, edges or vertices first.", cx);
            return;
        }
        let picked = selection.all_vertices(&faces);
        let pivot = math::scale(picked.iter().fold([0.0; 3], |a, v| math::add(a, verts.get(*v as usize).copied().unwrap_or_default())), 1.0 / picked.len().max(1) as f64);
        let picked: HashSet<_> = picked.into_iter().collect();
        let selected_faces: HashSet<_> = selection.faces.iter().copied().collect();
        let chosen: Vec<_> = faces.iter().enumerate().filter(|(i, f)| {
            selected_faces.contains(&(*i as u32)) || (selected_faces.is_empty() && f.iter().all(|v| picked.contains(v)))
        }).map(|(_, f)| f.as_slice()).collect();
        let normal = selected_normal(&verts, &chosen).unwrap_or_else(|| view.toward_viewer(pivot));
        // Picking supplies world-space vertices; editMesh translates the mesh's local data.
        // Keep the inverse scale as well as rotation so typed distances remain world units.
        let Some((_, _, Scene::Space(scene), time)) = self.scene(cx) else { return };
        let world = self.studio.read(cx).worlds(&scene,time,cx).get(&id).copied().unwrap_or(math::IDENTITY);
        let linear = std::array::from_fn(|r| std::array::from_fn(|c| world[c][r]));
        let Some(normal) = math::m3_inverse(&linear).map(|inverse| math::m3_apply(&inverse, normal))
            .filter(|n| n.iter().all(|v| v.is_finite()) && math::len(*n) > 0.)
        else {
            super::super::flash("Restore a nonzero, finite mesh scale before pulling its faces.", cx);
            return;
        };
        let key = self.studio.update(cx, |s, _| s.drag_key());
        self.mesh_modal = Some(MeshModal {
            kind, mouse0: self.mouse, normal, unit: view.units_per_pixel(pivot), typed: String::new(),
            valid: true, amount: 0.0, applied: 0.0, busy: false,
            key, clip, id, selection, origin: None, finish: None,
        });
        self.intercept(cx);
        if kind == ModalKind::Extrude {
            self.send_mesh_modal("extrude", json!({ "distance": 0 }), 0., false, cx);
        }
        self.studio.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub(super) fn mesh_modal_to(&mut self, mouse: [f64; 2], cx: &mut Context<Self>) {
        let snap = self.studio.read(cx).snapping;
        let Some(m) = self.mesh_modal.as_mut() else { return };
        // A click/Enter fixes the final amount; later pointer movement cannot change it.
        if m.finish.is_some() { return; }
        let moved = (mouse[0] - m.mouse0[0]).hypot(mouse[1] - m.mouse0[1]);
        let typed = m.typed.parse::<f64>().ok().filter(|n| n.is_finite());
        m.valid = m.typed.is_empty() || typed.is_some();
        if !m.valid { cx.notify(); return; }
        let amount = typed.unwrap_or_else(|| {
            let v = match m.kind {
                ModalKind::Extrude => (m.mouse0[1] - mouse[1]) * m.unit,
                _ => moved * m.unit * 0.5,
            };
            if snap { math::snap(v, 0.05) } else { v }
        });
        let offset = math::scale(m.normal, amount);
        m.valid = amount.is_finite() && offset.iter().all(|n| n.is_finite())
            && m.origin.as_ref().is_none_or(|origin| origin.positions.iter().all(|(_, p)| math::add(*p, offset).iter().all(|n| n.is_finite())));
        if m.valid {
            m.amount = amount;
            self.advance_mesh_modal(cx);
        }
        cx.notify();
    }

    pub(super) fn finish_mesh_modal(&mut self, finish: MeshFinish, cx: &mut Context<Self>) {
        let Some(m) = self.mesh_modal.as_mut() else { return };
        if finish == MeshFinish::Confirm && (!m.valid || m.finish.is_some()) { return; }
        // Inset/bevel commit once on Enter; their submitted command has no live pull to undo.
        if m.busy && m.kind != ModalKind::Extrude { return; }
        m.finish = Some(finish);
        self.advance_mesh_modal(cx);
        cx.notify();
    }

    fn clear_mesh_modal(&mut self, cx: &mut Context<Self>) {
        self.mesh_modal = None;
        self._intercept = None;
        self.studio.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    fn advance_mesh_modal(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.mesh_modal.as_ref() else { return };
        if m.busy || (!m.valid && m.finish != Some(MeshFinish::Cancel)) { return; }
        if m.kind == ModalKind::Extrude {
            let amount = if m.finish == Some(MeshFinish::Cancel) { 0. } else { m.amount };
            if amount != m.applied {
                self.send_mesh_modal(MESH_MOVE, json!({}), amount, false, cx);
                return;
            }
            if m.finish.is_none() { return; }
        } else if m.finish == Some(MeshFinish::Confirm) {
            let (op, params) = match m.kind {
                ModalKind::Inset => ("inset", json!({ "thickness": m.amount.abs().max(0.001) })),
                ModalKind::Bevel => ("bevel", json!({ "width": m.amount.abs().max(0.001), "segments": 1 })),
                _ => return,
            };
            self.send_mesh_modal(op, params, m.amount, true, cx);
            return;
        } else if m.finish.is_none() {
            return;
        }
        self.clear_mesh_modal(cx);
    }

    /// The mesh, components and gesture key stay fixed even if the sidebar selection changes.
    /// `applied` changes only on success, so a rejected command is never counted as geometry.
    fn send_mesh_modal(&mut self, op: &str, params: Value, applied: f64, terminal: bool, cx: &mut Context<Self>) {
        let Some(m) = self.mesh_modal.as_mut() else { return };
        let key = m.key.clone();
        let generation = self.scene_generation;
        let initial = op == "extrude";
        let pulling = op == MESH_MOVE;
        let (command, p) = if pulling {
            let Some(origin) = &m.origin else { return };
            let offset = math::scale(m.normal, applied);
            let vertices: Vec<_> = origin.positions.iter().map(|(index, p)| {
                json!({"index":index,"position":if applied==0. {*p} else {math::add(*p,offset)}})
            }).collect();
            ("motion.updateMeshVertices", json!({"clipId":m.clip,"id":m.id,"vertices":vertices,
                "expectedVertexCount":origin.vertex_count,"coalesce":key}))
        } else {
            let selection = m.selection.params();
            ("motion.editMesh", json!({ "clipId": m.clip, "id": m.id, "op": op, "params": params, "coalesce": key,
                "vertices": selection["vertices"], "edges": selection["edges"], "faces": selection["faces"] }))
        };
        m.busy = true;
        let task = super::super::call(command, p, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.scene_generation != generation || !this.mesh_modal.as_ref().is_some_and(|m| m.key == key) { return; }
                match result {
                    Ok(value) => {
                        let selection = (!pulling).then(|| super::super::selection_from(&value));
                        let origin = if initial {
                            let m = this.mesh_modal.as_ref().expect("matched gesture");
                            let origin = this.mesh_origin(m.clip, &m.id, selection.as_ref().expect("initial selection"), cx);
                            if origin.is_none() {
                                this.clear_mesh_modal(cx);
                                super::super::flash("The mesh changed while extrusion was starting. Undo and try again.", cx);
                                return;
                            }
                            origin
                        } else { None };
                        let m = this.mesh_modal.as_mut().expect("matched gesture");
                        m.busy = false;
                        m.applied = applied;
                        if initial { m.origin = origin; }
                        if let Some(selection) = selection {
                            this.studio.update(cx, |s, cx| {
                                if s.clip == Some(m.clip) && s.active() == Some(m.id.as_str()) && s.mode == Mode::Edit {
                                    s.edit_sel = selection.clone();
                                    s.changed(cx);
                                }
                            });
                            m.selection = selection;
                        }
                        if terminal { this.clear_mesh_modal(cx); } else { this.advance_mesh_modal(cx); }
                    }
                    Err(error) => {
                        if initial || this.mesh_modal.as_ref().is_some_and(|m|m.finish==Some(MeshFinish::Cancel)) {
                            // A changed topology can make restoration impossible. End the tool
                            // with the command's error so it cannot trap the user in a retry loop.
                            this.clear_mesh_modal(cx);
                        } else if let Some(m) = this.mesh_modal.as_mut() {
                            // Keep the last successful preview and wait for correction or Esc.
                            m.busy = false;
                            m.valid = false;
                            m.finish = None;
                        }
                        this.store.update(cx, |s, cx| s.error(error, cx));
                        cx.notify();
                    }
                }
            }).ok();
        }).detach();
        self.interacting(cx);
    }

    fn mesh_origin(&self, clip: Id, id: &str, selection: &super::super::EditSel, cx: &App) -> Option<MeshOrigin> {
        let store = self.store.read(cx);
        let kimchi_core::ClipContent::Motion {scene: Scene::Space(scene), ..} = &store.clip(clip)?.content else {return None};
        let object = kimchi_core::motion::find_object(&scene.objects, id)?;
        let kimchi_core::motion::Shape3d::Mesh {vertices, faces, ..} = &object.shape else {return None};
        let positions = selection.all_vertices(faces).into_iter().map(|i| vertices.get(i as usize).map(|p| (i,*p))).collect::<Option<Vec<_>>>()?;
        (!positions.is_empty()).then_some(MeshOrigin {vertex_count:vertices.len(), positions})
    }
}

/// Area-weighted polygon normals with a shared scale, so tiny faces don't underflow or
/// large faces overflow. A full polygon fan also handles collinear first vertices.
fn selected_normal(vertices: &[V3], faces: &[&[u32]]) -> Option<V3> {
    let extent = faces.iter().filter(|f| f.len() >= 3).flat_map(|f| {
        let first = vertices[f[0] as usize];
        f.iter().map(move |i| math::sub(vertices[*i as usize], first))
    }).flatten().map(f64::abs).fold(0., f64::max);
    if extent == 0. || !extent.is_finite() { return None; }
    let mut sum = [0.; 3];
    for face in faces.iter().filter(|f| f.len() >= 3) {
        let first = vertices[face[0] as usize];
        let edge = |i: u32| math::sub(vertices[i as usize], first).map(|v| v / extent);
        for pair in face[1..].windows(2) {
            sum = math::add(sum, math::cross(edge(pair[0]), edge(pair[1])));
        }
    }
    let normal = math::norm(sum);
    (normal != [0.; 3]).then_some(normal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_normals_preserve_winding_and_area_at_different_scales() {
        // The first three vertices are collinear; the remaining polygon still has area.
        let polygon = [[0.,0.,0.], [1.,0.,0.], [2.,0.,0.], [2.,0.,2.], [0.,0.,2.]];
        for size in [1e-200, 1e-8, 1., 1e200] {
            let vertices: Vec<_> = polygon.iter().map(|p| math::scale(*p,size)).collect();
            assert_eq!(selected_normal(&vertices,&[&[0,1,2,3,4]]),Some([0.,-1.,0.]));
            assert_eq!(selected_normal(&vertices,&[&[4,3,2,1,0]]),Some([0.,1.,0.]));
        }
        let vertices = [[0.,0.,0.],[2.,0.,0.],[0.,2.,0.],[0.,0.,1.]];
        let normal = selected_normal(&vertices,&[&[0,1,2],&[0,3,1]]).unwrap();
        assert!(math::len(math::sub(normal,math::norm([0.,2.,4.])))<1e-12);
        assert_eq!(selected_normal(&vertices,&[&[0,1,1]]),None);
    }
}
