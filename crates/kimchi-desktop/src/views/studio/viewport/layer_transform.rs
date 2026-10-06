//! Values for a 2D layer gesture, always recomputed from its initial state.

use super::{LayerDrag, LayerOp, math, model, round2};
use serde_json::{Map, Value, json};

impl LayerDrag {
    pub(super) fn update(&mut self, mouse: [f64; 2], shift: bool) -> bool {
        let typed = self.typed.parse::<f64>().ok().filter(|n| n.is_finite());
        self.valid = self.typed.is_empty() || typed.is_some();
        if !self.valid {
            return false;
        }
        let position = |v| if typed.is_some() { v } else { round2(v) };
        let mut reached = vec![];
        for st in &self.starts {
            let mut props = Map::new();
            let anchor = math::aff_apply(&st.world, st.anchor);
            match self.op {
                LayerOp::Move => {
                    let mut delta = [mouse[0] - self.mouse0[0], mouse[1] - self.mouse0[1]];
                    if shift {
                        if delta[0].abs() > delta[1].abs() {
                            delta[1] = 0.;
                        } else {
                            delta[0] = 0.;
                        }
                    }
                    let local = math::aff_invert(&st.parent)
                        .map(|inv| math::aff_dir(&inv, delta))
                        .unwrap_or(delta);
                    props.insert("x".into(), json!(position(st.x + local[0])));
                    props.insert("y".into(), json!(position(st.y + local[1])));
                }
                LayerOp::Rotate => {
                    let deg = typed.unwrap_or_else(|| {
                        let a0 = (self.mouse0[1] - anchor[1]).atan2(self.mouse0[0] - anchor[0]);
                        let a1 = (mouse[1] - anchor[1]).atan2(mouse[0] - anchor[0]);
                        let deg = (a1 - a0).to_degrees();
                        if shift { math::snap(deg, 15.) } else { deg }
                    });
                    props.insert("rotation".into(), json!(position(st.rotation + deg)));
                }
                LayerOp::Scale(hx, hy) => {
                    let (hx, hy) = match self.axis {
                        Some(0) => (1, 0),
                        Some(1) => (0, 1),
                        _ => (hx, hy),
                    };
                    let (mut fx, mut fy) = if let Some(k) = typed {
                        (if hx != 0 { k } else { 1. }, if hy != 0 { k } else { 1. })
                    } else {
                        let Some(inv) = math::aff_invert(&st.world) else {
                            continue;
                        };
                        let a = math::aff_apply(&inv, self.mouse0);
                        let b = math::aff_apply(&inv, mouse);
                        let ratio = |axis: usize| {
                            let from = a[axis] - st.anchor[axis];
                            if from == 0. {
                                1.
                            } else {
                                (b[axis] - st.anchor[axis]) / from
                            }
                        };
                        (
                            if hx != 0 { ratio(0) } else { 1. },
                            if hy != 0 { ratio(1) } else { 1. },
                        )
                    };
                    if shift && typed.is_none() && self.axis.is_none() {
                        let k = if hx != 0 && hy != 0 {
                            (fx + fy) / 2.
                        } else if hx != 0 {
                            fx
                        } else {
                            fy
                        };
                        fx = k;
                        fy = k;
                    }
                    if fx == fy && self.axis.is_none() {
                        props.insert("scale".into(), json!(st.scale * fx));
                    } else {
                        if hx != 0 {
                            props.insert("scaleX".into(), json!(st.scale_x * fx));
                        }
                        if hy != 0 {
                            props.insert("scaleY".into(), json!(st.scale_y * fy));
                        }
                    }
                }
                LayerOp::Anchor => {
                    let Some(inv) = math::aff_invert(&st.world) else {
                        continue;
                    };
                    let a = math::aff_apply(&inv, mouse);
                    let own = math::layer_affine(
                        0.,
                        0.,
                        st.rotation,
                        0.,
                        st.scale * st.scale_x,
                        st.scale * st.scale_y,
                        0.,
                        0.,
                    );
                    let shift = math::aff_dir(&own, [a[0] - st.anchor[0], a[1] - st.anchor[1]]);
                    props.insert("anchorX".into(), json!(round2(a[0])));
                    props.insert("anchorY".into(), json!(round2(a[1])));
                    props.insert("x".into(), json!(round2(st.x + shift[0])));
                    props.insert("y".into(), json!(round2(st.y + shift[1])));
                }
            }
            // JSON represents an overflowing float as null; do not send any partial selection.
            if props
                .values()
                .any(|v| v.as_f64().is_none_or(|n| !n.is_finite()))
            {
                self.valid = false;
                return false;
            }
            if matches!(self.op, LayerOp::Scale(..)) {
                let factor = |name: &str, original: f64| {
                    props.get(name).and_then(Value::as_f64).unwrap_or(original)
                };
                let k = factor("scale", st.scale);
                if !(k * factor("scaleX", st.scale_x)).is_finite()
                    || !(k * factor("scaleY", st.scale_y)).is_finite()
                {
                    self.valid = false;
                    return false;
                }
            }
            reached.push((st.id.clone(), props));
        }
        for (id, props) in &mut reached {
            let st = self
                .starts
                .iter_mut()
                .find(|s| s.id == *id)
                .expect("starting layer");
            // Switching uniform/axis scaling must undo channels from the previous preview,
            // including their curves, before applying the new one from the same starting state.
            let unused = st
                .touched
                .iter()
                .filter(|(name, _)| !props.contains_key(*name))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Map<String, Value>>();
            st.touched.extend(props.clone());
            if !unused.is_empty() {
                props.extend(model::restore_props(&st.base, &st.keys, &unused));
            }
        }
        self.reached = reached;
        true
    }
}
