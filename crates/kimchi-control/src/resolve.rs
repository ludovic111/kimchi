//! Ids or names: wherever a command takes `clipId`, `trackId`, `assetId`,
//! `markerId` or `projectId`, a unique name works too. A wrong or ambiguous
//! name is refused with what exists and the closest match.

use kimchi_core::{Id, Project, ProjectSummary};

use crate::registry::closest;
use crate::session::CmdResult;

fn pick(what: &str, key: &str, items: &[(Id, String)], lister: &str) -> CmdResult<Id> {
    let key = key.trim();
    if let Ok(id) = key.parse::<Id>() {
        if items.iter().any(|(i, _)| *i == id) {
            return Ok(id);
        }
        return Err(format!("No {what} has the id {id}. {lister} lists them."));
    }
    let matches: Vec<&(Id, String)> = items.iter().filter(|(_, n)| n.eq_ignore_ascii_case(key)).collect();
    match matches.as_slice() {
        [one] => Ok(one.0),
        [] => {
            let names: Vec<&str> = items.iter().map(|(_, n)| n.as_str()).collect();
            let hint = closest(key, &names).map(|c| format!(" Did you mean {c}?")).unwrap_or_default();
            let some: Vec<&str> = names.iter().take(12).copied().collect();
            let list = if some.is_empty() { format!("There are no {what}s.") } else { format!("Known: {}.", some.join(", ")) };
            Err(format!("Unknown {what} `{key}`.{hint} {list}"))
        }
        many => Err(format!(
            "{} {what}s are named `{key}`; use an id instead: {}.",
            many.len(),
            many.iter().map(|(i, _)| i.to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

pub fn clip(p: &Project, key: &str) -> CmdResult<Id> {
    let items: Vec<(Id, String)> = p.clips().map(|(_, c)| (c.id, c.name.clone())).collect();
    pick("clip", key, &items, "clip.list")
}

pub fn clips(p: &Project, keys: &[String]) -> CmdResult<Vec<Id>> {
    keys.iter().map(|k| clip(p, k)).collect()
}

pub fn track(p: &Project, key: &str) -> CmdResult<Id> {
    let items: Vec<(Id, String)> = p.tracks.iter().map(|t| (t.id, t.name.clone())).collect();
    pick("track", key, &items, "track.list")
}

pub fn asset(p: &Project, key: &str) -> CmdResult<Id> {
    let items: Vec<(Id, String)> = p.assets.iter().map(|a| (a.id, a.name.clone())).collect();
    pick("media item", key, &items, "media.list")
}

pub fn marker(p: &Project, key: &str) -> CmdResult<Id> {
    let items: Vec<(Id, String)> = p.markers.iter().map(|m| (m.id, m.label.clone())).collect();
    pick("marker", key, &items, "timeline.markers")
}

pub fn project(list: &[ProjectSummary], key: &str) -> CmdResult<Id> {
    let items: Vec<(Id, String)> = list.iter().map(|p| (p.id, p.name.clone())).collect();
    pick("project", key, &items, "project.list")
}
