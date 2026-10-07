//! MCP prompts: one per harness skill (`harness.skills`), so a client can start a job with
//! kimchi's playbook for it. The names of the prompts kimchi had before the skills still work.

use kimchi_control::harness;
use serde_json::{Value, json};

/// Prompt names from before the skills, and the skill each now is.
const ALIASES: &[(&str, &str)] =
    &[("title-and-captions", "titles-and-captions"), ("generate-b-roll", "b-roll"), ("motion-design", "motion-graphics"), ("3d-scene", "3d-product-shot")];

/// `prompts/list`.
pub fn list() -> Vec<Value> {
    harness::skills()
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "title": s.title,
                "description": format!("{} {}", s.title, s.when).trim().to_string(),
                "arguments": [{ "name": "request", "description": "What the person wants, in their own words (optional).", "required": false }],
            })
        })
        .collect()
}

/// `prompts/get`: the skill's playbook, the request and any other arguments given, and the
/// finish routine's reminder. `None` for an unknown name.
pub fn get(name: &str, arguments: &Value) -> Option<Value> {
    let skill = ALIASES.iter().find(|(old, _)| *old == name).map_or(name, |(_, new)| new);
    let info = harness::skills().into_iter().find(|s| s.name == skill)?;
    let text = harness::as_tools(harness::skill(skill).ok()?);
    let mut asked: Vec<String> = vec![];
    if let Some(r) = arguments.get("request").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty()) {
        asked.push(r.to_string());
    }
    // Arguments of the older prompts (title, length, subject…) are passed on as they are.
    for (k, v) in arguments.as_object().into_iter().flatten().filter(|(k, _)| *k != "request") {
        let v = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
        if !v.trim().is_empty() {
            asked.push(format!("{k}: {}", v.trim()));
        }
    }
    let request = if asked.is_empty() { "Ask the person what they want if it isn't clear from the project.".to_string() } else { format!("The request:\n{}", asked.join("\n")) };
    Some(json!({
        "description": format!("{} {}", info.title, info.when).trim().to_string(),
        "messages": [{ "role": "user", "content": { "type": "text", "text": format!(
            "Use kimchi's playbook for this job.\n\n{text}\n\n{request}\n\nStart with project_overview. Before you say it is done, run the finish routine: harness_look, compare with the request, fix (up to three passes), then report in a few lines."
        ) } }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_is_a_prompt_and_old_names_still_work() {
        let names: Vec<String> = list().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"rough-cut".to_string()) && names.len() >= 8);
        let p = get("rough-cut", &json!({ "request": "a 30 s cut of the beach footage" })).unwrap();
        let text = p["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("# Rough cut") && text.contains("a 30 s cut of the beach footage") && text.contains("harness_look"), "{text}");
        let old = get("title-and-captions", &json!({ "title": "Hello" })).unwrap();
        assert!(old["messages"][0]["content"]["text"].as_str().unwrap().contains("title: Hello"));
        assert!(get("nope", &json!({})).is_none());
    }
}
