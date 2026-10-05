//! Effects: ryolune's stock effects and the CLAP, VST3, Audio Unit and ryolune native plugins
//! found by ryolune's own scan (its cache is shared, so both apps list the same plugins).

use serde::Serialize;

use crate::Result;

/// An effect that can go in a chain.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectInfo {
    /// Descriptor id stored in `Insert::plugin` (`stock:Channel EQ`, `clap:…`, `vst3:…`).
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// `ryolune` (stock), `Native`, `CLAP`, `VST3` or `AU`.
    pub format: String,
    /// Dynamics, EQ & Filter, Distortion, Modulation, Pitch, Space & Time, Utility…
    pub category: String,
    pub description: String,
}

/// One parameter of an effect (plain values: dB, Hz, %, ms).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub unit: String,
    /// 0 for continuous parameters, else the number of steps.
    pub steps: u32,
    pub log: bool,
    pub labels: Vec<String>,
}

impl ParamInfo {
    /// The value as a person reads it ("-6.0 dB", "Hall").
    pub fn text(&self, value: f64) -> String {
        to_ryolune(self).text(value)
    }

    /// A typed value back ("-6 dB", "2.5k", "Hall", "50%"); `None` when it means nothing here.
    pub fn parse(&self, text: &str) -> Option<f64> {
        to_ryolune(self).parse_text(text)
    }

    /// 0..1 along the control's travel (logarithmic for frequencies).
    pub fn normalize(&self, value: f64) -> f64 {
        to_ryolune(self).normalize(value)
    }

    pub fn denormalize(&self, t: f64) -> f64 {
        to_ryolune(self).denormalize(t)
    }
}

fn to_ryolune(p: &ParamInfo) -> ryolune_engine::plugin::ParamInfo {
    ryolune_engine::plugin::ParamInfo {
        id: p.id,
        name: p.name.clone(),
        min: p.min,
        max: p.max,
        default: p.default,
        unit: p.unit.clone(),
        steps: p.steps,
        log: p.log,
        labels: p.labels.clone(),
    }
}

fn from_ryolune(p: ryolune_engine::plugin::ParamInfo) -> ParamInfo {
    ParamInfo { id: p.id, name: p.name, min: p.min, max: p.max, default: p.default, unit: p.unit, steps: p.steps, log: p.log, labels: p.labels }
}

/// Every effect kimchi can use, stock first, matching `query` (name, vendor, category) when given.
pub fn effects(query: Option<&str>) -> Vec<EffectInfo> {
    let q = query.map(str::to_lowercase).filter(|q| !q.trim().is_empty());
    ryolune_engine::stock::descriptors()
        .into_iter()
        .filter(|d| d.effect && !d.instrument)
        .map(|d| EffectInfo {
            description: ryolune_engine::stock::description(&d.name).unwrap_or_default().to_string(),
            id: d.id,
            name: d.name,
            vendor: d.vendor,
            format: d.format.label().to_string(),
            category: d.category,
        })
        .filter(|e| q.as_ref().is_none_or(|q| [&e.name, &e.vendor, &e.category].iter().any(|f| f.to_lowercase().contains(q.as_str()))))
        .collect()
}

/// One effect by id or name (case-insensitive), with a "did you mean" error.
pub fn effect(id_or_name: &str) -> Result<EffectInfo> {
    let all = effects(None);
    if let Some(e) = all.iter().find(|e| e.id == id_or_name || e.name.eq_ignore_ascii_case(id_or_name)) {
        return Ok(e.clone());
    }
    let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
    let hint = kimchi_core::closest(id_or_name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("There is no effect `{id_or_name}`.{hint} audio.effects lists them."))
}

/// An effect's parameters.
pub fn params(plugin_id: &str) -> Result<Vec<ParamInfo>> {
    let e = effect(plugin_id)?;
    match e.id.strip_prefix("stock:") {
        Some(name) => Ok(ryolune_engine::stock::params(name).into_iter().map(from_ryolune).collect()),
        None => {
            let instance = ryolune_engine::host::instantiate(&e.id, &e.name, crate::SAMPLE_RATE)?;
            Ok(instance.editor.params().iter().cloned().map(from_ryolune).collect())
        }
    }
}

/// A parameter by its id or name (case-insensitive), with a "did you mean" error.
pub fn param(plugin_id: &str, id_or_name: &str) -> Result<ParamInfo> {
    let all = params(plugin_id)?;
    if let Some(p) = all.iter().find(|p| p.id.to_string() == id_or_name || p.name.eq_ignore_ascii_case(id_or_name)) {
        return Ok(p.clone());
    }
    let names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
    let hint = kimchi_core::closest(id_or_name, &names).map(|c| format!(" Did you mean `{c}`?")).unwrap_or_default();
    Err(format!("{plugin_id} has no parameter `{id_or_name}`.{hint} Parameters: {}.", names.join(", ")))
}

/// A new slot for `plugin` (id or name) with its default settings.
pub fn new_insert(plugin: &str, slot_id: impl Into<String>) -> Result<kimchi_core::Insert> {
    let e = effect(plugin)?;
    Ok(kimchi_core::Insert::new(slot_id, &e.id, &e.name))
}

/// Looks for plugins again (CLAP, VST3, Audio Units, native) in the standard folders, through
/// ryolune's crash-isolated scanner, and returns how many effects are known afterwards.
pub fn rescan() -> Result<usize> {
    Err("Scanning for plugins isn't built yet.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_effects_are_listed_with_their_parameters() {
        let all = effects(None);
        assert!(all.len() >= 20, "{all:?}");
        let eq = effect("channel eq").unwrap();
        assert_eq!(eq.id, "stock:Channel EQ");
        assert!(!params(&eq.id).unwrap().is_empty());
        assert!(effect("Chanel EQ").unwrap_err().contains("Channel EQ"));
        assert!(effects(Some("dynamics")).iter().any(|e| e.name == "Limiter"));
    }
}
