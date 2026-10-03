//! A scene at an instant: what the renderers draw. Keyframes first, then expressions (formulas
//! that can read time, the keyframed value and other things' properties), then constraints
//! (lookAt, followPath…), so a constraint always has the last word.

use super::*;

impl Scene2d {
    /// The scene's layers at scene time `t`.
    pub fn layers_at(&self, t: f64) -> Vec<Layer> {
        self.layers.iter().map(|l| l.at(t)).collect()
    }

    /// A composition's layers at its own time `t` (None when there is no such composition).
    pub fn comp_layers_at(&self, comp: &str, t: f64) -> Option<Vec<Layer>> {
        Some(self.composition(comp)?.layers.iter().map(|l| l.at(t)).collect())
    }
}

impl Scene3d {
    /// The objects at scene time `t`, children included, with shared materials put in place.
    pub fn objects_at(&self, t: f64) -> Vec<Object3d> {
        let mut out: Vec<Object3d> = self.objects.iter().map(|o| o.at(t)).collect();
        walk_objects_mut(&mut out, &mut |o| {
            if o.material.from.is_some() {
                let keyed = o.material.clone();
                let mut m = self.resolve_material(&o.material);
                // The object's own keyframed material values stay on top of the shared one.
                for name in o.keyframes.keys().chain(self.objects_keyframe_names(&o.id).iter()) {
                    if let Some(v) = keyed.get_value(name) {
                        let _ = m.set_value(name, &v);
                    }
                }
                o.material = m;
            }
        });
        out
    }

    fn objects_keyframe_names(&self, id: &str) -> Vec<String> {
        find_object(&self.objects, id).map(|o| o.keyframes.keys().cloned().collect()).unwrap_or_default()
    }

    /// The lights at scene time `t`.
    pub fn lights_at(&self, t: f64) -> Vec<Light> {
        self.lights.iter().filter(|l| !l.hidden).map(|l| l.at(t)).collect()
    }

    /// The camera filming at scene time `t`.
    pub fn camera_at(&self, t: f64) -> Camera {
        let id = self.active_camera_at(t);
        self.camera_by_id(&id).unwrap_or(&self.camera).at(t)
    }
}

impl Material {
    /// A material property by keyframe name (`color`, `roughness`, `pattern.scale`…).
    pub fn get_value(&self, name: &str) -> Option<KeyValue> {
        self.get(name)
    }

    pub fn set_value(&mut self, name: &str, v: &KeyValue) -> Result<bool, String> {
        self.set(name, v)
    }
}
