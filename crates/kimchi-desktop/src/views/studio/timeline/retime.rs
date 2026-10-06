//! Keyboard keyframe transforms preview locally, then commit through the command registry.
use super::*;

fn amount_label(value: f64) -> String {
    if value==0. {return "0".into();}
    if value.abs()<0.001 || value.abs()>=1e6 {return format!("{value:.3e}");}
    let precision=if value.abs()<1. {6} else {3};
    format!("{value:.precision$}").trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(super) struct TimeTransform {
    clip: kimchi_core::Clip,
    sources: Vec<(KeyRef, Keyframe)>,
    channels: std::collections::HashMap<(String,String),Vec<Keyframe>>,
    pub next: Vec<KeyRef>,
    values: Vec<KeyValue>,
    scale: bool,
    mouse0: Point<Pixels>,
    pointer: [f64;2],
    graph: bool,
    value_axis: bool,
    value_available: bool,
    component: usize,
    component_label: String,
    value_span: f64,
    typed: String,
    fps: f64,
    free: bool,
    pub label: String,
    valid: bool,
}

impl TimeTransform {
    fn refresh(&mut self) {
        let amount = if self.typed.is_empty() { Some(self.pointer[usize::from(self.value_axis)]) } else { self.typed.parse::<f64>().ok() };
        let amount = amount.filter(|n| n.is_finite() && (!self.scale || self.value_axis || *n > 0.));
        self.valid = amount.is_some();
        self.next = self.sources.iter().map(|(key,_)| key.clone()).collect();
        self.values=self.sources.iter().map(|(_,key)| key.value.clone()).collect();
        let action=if self.value_axis {format!("{} {}values",if self.scale {"Scale"} else {"Move"},self.component_label)}
            else {if self.scale {"Scale key timing"} else {"Move keys"}.to_string()};
        let axes=if self.graph {" · X time / Y values"} else {""};
        if self.value_axis && !self.value_available {
            self.valid=false;
            self.label="Select keys on the visible numeric curve for values · X returns to time · Esc cancels".into();
            return;
        }
        let Some(amount)=amount else {
            self.label=format!("{action} · {} · enter {}{axes} · Esc cancels",self.typed,if self.scale && !self.value_axis {"a positive factor"} else {"a number"});
            return;
        };
        // Keep exactly what the person typed visible, including small values and exponents.
        let entered=if self.typed.is_empty() {amount_label(amount)} else {self.typed.clone()};
        if self.value_axis {
            for value in &mut self.values {
                let number=match value {KeyValue::Number(n)=>Some(n),KeyValue::Vector(v)=>v.get_mut(self.component),_=>None};
                let Some(number)=number else {self.valid=false; continue};
                *number=if self.scale {*number*amount} else {*number+amount};
                self.valid &= number.is_finite();
            }
            self.label=if self.valid {format!("{action} · {entered}{}{axes} · Enter applies · Esc cancels",if self.scale {"×"} else {""})}
                else {"Value exceeds the numeric range · Esc cancels".into()};
            if !self.valid {self.values=self.sources.iter().map(|(_,key)| key.value.clone()).collect();}
            return;
        }
        let first=self.next.iter().map(|k| k.time).fold(f64::INFINITY,f64::min);
        let mut delta=amount*self.clip.speed/self.fps;
        if !self.scale {
            if self.typed.is_empty() && amount.abs()>1e-9 { delta=snap_key_time(&self.clip,first+delta,self.fps,self.free)-first; }
            delta=key_delta(&self.next,delta);
        }
        for key in &mut self.next {
            key.time=if self.scale {first+(key.time-first)*amount} else {key.time+delta};
            self.valid &= key.time.is_finite() && key.time <= MAX_TIME;
        }
        if !self.valid {self.next=self.sources.iter().map(|(key,_)| key.clone()).collect();}
        let value=if self.scale {format!("{entered}× from first key")} else {
            let frames=if !self.typed.is_empty() && delta==amount*self.clip.speed/self.fps {entered}
                else {amount_label(delta*self.fps/self.clip.speed)};
            format!("{frames} frames")
        };
        self.label=if self.valid {format!("{action} · {value}{axes} · Enter applies · Esc cancels")}
            else {format!("{action} · time exceeds the editable range · Esc cancels")};
    }

    pub fn time(&self, key: &KeyRef) -> f64 {
        self.sources.iter().position(|(source,_)| source==key).map_or(key.time,|i| self.next[i].time)
    }

    pub fn curve(&self, id: &str, property: &str, list: &[Keyframe]) -> Vec<Keyframe> {
        let selected=|time:f64| self.sources.iter().any(|(k,_)| k.id==id && k.property==property && (k.time-time).abs()<1e-6);
        let mut preview:Vec<_>=list.iter().filter(|k| !selected(k.time)).cloned().collect();
        for (index,((_,key),target)) in self.sources.iter().zip(&self.next).enumerate().filter(|(_,((k,_),_))| k.id==id && k.property==property) {
            preview.retain(|k| (k.time-target.time).abs()>=1e-6);
            preview.push(Keyframe {time:target.time,value:self.values[index].clone(),..key.clone()});
        }
        preview.sort_by(|a,b| a.time.total_cmp(&b.time));
        preview
    }
}

impl StudioTimeline {
    pub fn start_retime(&mut self, scale: bool, window: &Window, cx: &mut Context<Self>) {
        self.cancel_drag(cx);
        let st=self.studio.read(cx);
        let Some((clip,scene))=st.clip_scene(cx) else {return};
        if st.keys.is_empty() {return;}
        let mut sources=vec![];
        let mut channels=std::collections::HashMap::new();
        for key in &st.keys {
            let keys=channels.entry((key.id.clone(),key.property.clone())).or_insert_with(|| model::keyframes(&scene,&key.id)
                .and_then(|mut keys| keys.remove(&key.property)).unwrap_or_default());
            let Some(source)=keys.iter().find(|k| (k.time-key.time).abs()<1e-6).cloned() else {return};
            sources.push((key.clone(),source));
        }
        let graph=self.graph_target(&scene,cx);
        let dimensions=graph.as_ref().and_then(|(id,p)| channels.get(&(id.clone(),p.clone()))).and_then(|k| k.first())
            .map(|k| values(Some(k.value.clone())).len()).unwrap_or(1).max(1);
        let component=st.graph_component.min(dimensions-1);
        let value_available=st.show_graph && sources.iter().all(|(key,k)| graph.as_ref().is_some_and(|(id,p)| *id==key.id && *p==key.property)
            && match &k.value {KeyValue::Number(_)=>true,KeyValue::Vector(v)=>component<v.len(),_=>false});
        let component_label=if dimensions>1 {format!("{} ",["X","Y","Z","W"].get(component).map(|s| s.to_string()).unwrap_or_else(|| (component+1).to_string()))} else {String::new()};
        let range=self.graph_range(&scene,cx);
        let value_span=range.1-range.0;
        let mut transform=TimeTransform {clip,sources,channels,next:vec![],values:vec![],scale,mouse0:window.mouse_position(),
            pointer:[if scale {1.} else {0.};2],graph:st.show_graph,value_axis:false,value_available,component,component_label,value_span,
            typed:String::new(),fps:st.store.read(cx).fps().max(1.),free:false,label:String::new(),valid:true};
        transform.refresh();
        self.transform=Some(transform);
        let me=cx.entity().downgrade();
        self.key_capture=Some(cx.intercept_keystrokes(move |event,_,cx| {
            let Some(me)=me.upgrade() else {return};
            if me.update(cx,|this,cx| this.retime_key(&event.keystroke,cx)) {cx.stop_propagation();}
        }));
        cx.notify();
    }

    fn retime_key(&mut self, key: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let Some(transform)=self.transform.as_mut() else {return false};
        match key.key.as_str() {
            "escape" => {self.cancel_drag(cx);}
            "enter" => self.finish_retime(cx),
            "backspace" => {transform.typed.pop(); transform.refresh();}
            "x" => {transform.value_axis=false; transform.refresh();}
            "y" if transform.graph => {transform.value_axis=true; transform.refresh();}
            _ => {
                let ch=key.key_char.as_deref().unwrap_or(&key.key);
                if ch.len()==1 && ch.chars().all(|c| c.is_ascii_digit() || matches!(c,'.'|'-'|'+'|'e'|'E')) {
                    transform.typed.push_str(ch);
                    transform.refresh();
                }
            }
        }
        cx.notify();
        true
    }

    pub(super) fn retime_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let span=self.span(cx);
        let width=(self.track_box().2-16.).max(1.);
        let height=(self.track_box().3-RULER_H as f64-12.).max(1.);
        let Some(transform)=self.transform.as_mut() else {return};
        let dx=f32::from(event.position.x-transform.mouse0.x) as f64;
        let dy=f32::from(transform.mouse0.y-event.position.y) as f64;
        transform.pointer=if transform.scale {[(dx/100.).exp2(),(dy/100.).exp2()]}
            else {[dx/width*(span.1-span.0)*transform.fps/transform.clip.speed,dy*transform.value_span/height]};
        transform.free=event.modifiers.alt;
        transform.refresh();
        cx.notify();
    }

    pub(super) fn finish_retime(&mut self, cx: &mut Context<Self>) {
        if self.transform.as_ref().is_none_or(|m| !m.valid) {return;}
        let transform=self.transform.take().expect("active retiming");
        self.key_capture=None;
        cx.notify();
        let Some((clip,scene))=self.studio.read(cx).clip_scene(cx) else {return};
        let selection=&self.studio.read(cx).keys;
        // A preview must never overwrite another client's intervening keyframe edit.
        if clip.id!=transform.clip.id || clip.start!=transform.clip.start || clip.in_point!=transform.clip.in_point
            || clip.speed!=transform.clip.speed || clip.reverse!=transform.clip.reverse || clip.duration!=transform.clip.duration
            || self.studio.read(cx).store.read(cx).fps().max(1.)!=transform.fps
            || !selection.iter().eq(transform.sources.iter().map(|(k,_)| k))
            || transform.channels.iter().any(|((id,property),source)| model::keyframes(&scene,id)
                .and_then(|mut keys| keys.remove(property)).as_ref()!=Some(source)) {
            super::super::flash("Keyframes changed while retiming. Start the edit again.",cx);
            return;
        }
        if transform.sources.iter().zip(&transform.next).zip(&transform.values).all(|(((key,source),target),value)| (key.time-target.time).abs()<1e-9 && source.value==*value) {return;}
        let updates=transform.sources.iter().zip(&transform.next).zip(&transform.values).map(|(((key,_),target),value)| {
            let mut update=json!({"id":key.id,"property":key.property,"time":model::timeline_time(&clip,key.time),"newTime":model::timeline_time(&clip,target.time)});
            if transform.value_axis {update["value"]=json!(value);}
            update
        }).collect();
        self.update_keys(clip.id,updates,transform.next,cx);
    }

    #[cfg(test)]
    pub fn retiming_for_test(&self) -> bool {self.transform.is_some()}
}
