//! Speakers and microphones: the lists for Settings › Audio, and the device a name stands for
//! (the setting's value), falling back to the system's default when it is gone.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

pub use cpal;

/// An audio device as Settings shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub name: String,
    /// The system's default for its direction.
    pub default: bool,
    /// Channels and sample rate of its default configuration.
    pub channels: u16,
    pub sample_rate: u32,
}

fn describe(d: &cpal::Device, default: Option<&str>, output: bool) -> Option<Device> {
    let name = d.name().ok()?;
    let config = if output { d.default_output_config() } else { d.default_input_config() }.ok();
    Some(Device {
        default: default == Some(name.as_str()),
        channels: config.as_ref().map_or(0, |c| c.channels()),
        sample_rate: config.as_ref().map_or(0, |c| c.sample_rate().0),
        name,
    })
}

/// Every output (speakers, headphones, interfaces), the default first.
pub fn outputs() -> Vec<Device> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.name().ok());
    let mut all: Vec<Device> = host.output_devices().map(|ds| ds.filter_map(|d| describe(&d, default.as_deref(), true)).collect()).unwrap_or_default();
    all.sort_by_key(|d| !d.default);
    all.dedup_by(|a, b| a.name == b.name);
    all
}

/// Every input (microphones, interfaces), the default first.
pub fn inputs() -> Vec<Device> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let mut all: Vec<Device> = host.input_devices().map(|ds| ds.filter_map(|d| describe(&d, default.as_deref(), false)).collect()).unwrap_or_default();
    all.sort_by_key(|d| !d.default);
    all.dedup_by(|a, b| a.name == b.name);
    all
}

/// The device called `name` (exactly, else ignoring case, else the one whose name contains
/// it), or the default one when there's no name or no such device.
fn pick(devices: impl Iterator<Item = cpal::Device>, default: Option<cpal::Device>, name: Option<&str>) -> Option<cpal::Device> {
    let Some(wanted) = name.map(str::trim).filter(|n| !n.is_empty()) else { return default };
    let named: Vec<(String, cpal::Device)> = devices.filter_map(|d| Some((d.name().ok()?, d))).collect();
    let lower = wanted.to_lowercase();
    let found = named
        .iter()
        .position(|(n, _)| n == wanted)
        .or_else(|| named.iter().position(|(n, _)| n.to_lowercase() == lower))
        .or_else(|| named.iter().position(|(n, _)| n.to_lowercase().contains(&lower)));
    match found {
        Some(i) => named.into_iter().nth(i).map(|(_, d)| d),
        None => {
            tracing::warn!(device = wanted, "audio device not found; using the default");
            default
        }
    }
}

/// The output device for the setting `name` (`None` or empty: the system's default).
pub fn output(name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let all = host.output_devices().ok()?;
    pick(all, host.default_output_device(), name)
}

/// The input device for the setting `name` (`None` or empty: the system's default).
pub fn input(name: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    let all = host.input_devices().ok()?;
    pick(all, host.default_input_device(), name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn listing_devices_never_fails() {
        // This machine may have no sound card: the lists are then empty, never an error.
        let outs = super::outputs();
        let ins = super::inputs();
        assert!(outs.iter().filter(|d| d.default).count() <= 1);
        assert!(ins.iter().all(|d| !d.name.is_empty()));
        // A name that isn't there falls back to the default (or nothing, without a card).
        let fallback = super::output(Some("no such device ✕"));
        assert_eq!(fallback.is_some(), super::output(None).is_some());
    }
}
