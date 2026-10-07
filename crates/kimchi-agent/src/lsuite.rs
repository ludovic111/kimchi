//! lsuite AI as an agent provider: the lsuite account (`kimchi_control::account`) runs the agent
//! on its plan, through the Anthropic provider pointed at `<server>/api/ai` with the account's
//! token as the key; and Claude Code can run on it too (`ANTHROPIC_BASE_URL`,
//! `ANTHROPIC_AUTH_TOKEN`) when the person picks that (`settings.agent.claudeCodeOnLsuite`).
//!
//! When the month's allowance runs out, the run ends with one line that starts with
//! [`ALLOWANCE`] and names where to manage the plan; the panel shows **Manage plan** next to it.
//! It never switches to another provider by itself.

use kimchi_control::account;

/// How an error about the allowance starts (the panel offers Manage plan for it).
pub const ALLOWANCE: &str = "Your lsuite AI allowance";

/// What a run says when nobody is signed in.
pub const SIGNED_OUT: &str = "Sign in to lsuite AI first: Settings › Agent › lsuite AI (or kimchi-cli account.signIn).";

/// An error from the lsuite server, said in one line for the person.
pub fn explain(error: &str) -> String {
    let status = error.split("error ").nth(1).and_then(|r| r.get(..3)).and_then(|c| c.parse::<u16>().ok());
    let lower = error.to_ascii_lowercase();
    let about_allowance = ["allowance", "credit", "quota", "limit reached", "used up", "exceeded"].iter().any(|w| lower.contains(w));
    let server = account::server();
    let manage = account::manage_url(&server);
    // The service's own message, without kimchi's hint about API keys.
    let said = error.split_once(": ").map(|(_, m)| m).unwrap_or(error).lines().next().unwrap_or("").trim();
    // The server's own line when it already says it all (lsuite.xyz does: plan, reset day, link).
    let with_manage = |text: &str| if text.contains("/account") { text.to_string() } else { format!("{} Manage plan: {manage}", text.trim_end_matches('.').to_string() + ".") };
    match status {
        Some(402 | 429 | 403) if said.starts_with(ALLOWANCE) => with_manage(said),
        Some(402) => format!("{ALLOWANCE} for this month is used up. Manage plan to get more: {manage}"),
        Some(429 | 403) if about_allowance => format!("{ALLOWANCE} for this month is used up. Manage plan to get more: {manage}"),
        Some(401) => "Your lsuite sign-in has expired or was revoked. Sign in again in Settings › Agent › lsuite AI.".to_string(),
        Some(403) => with_manage(said),
        _ => error.replace("\nCheck the API key in Settings › Agent.", "").replace("\nCheck the model name and address in Settings › Agent.", ""),
    }
}

/// The environment that runs Claude Code on lsuite AI, when the person chose it and is signed
/// in: `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`.
pub fn claude_code_env(settings: &kimchi_control::settings::AgentSettings) -> Option<[(&'static str, String); 2]> {
    if !settings.claude_code_on_lsuite {
        return None;
    }
    let a = account::load()?;
    Some([("ANTHROPIC_BASE_URL", account::ai_base(&a.server)), ("ANTHROPIC_AUTH_TOKEN", a.token)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_say_one_thing() {
        let e = explain("lsuite AI error 402: Monthly allowance used");
        assert!(e.starts_with(ALLOWANCE) && e.contains("/account") && !e.contains('\n'), "{e}");
        assert!(explain("lsuite AI error 429: credit limit exceeded").starts_with(ALLOWANCE));
        assert!(!explain("lsuite AI error 429: slow down").starts_with(ALLOWANCE));
        assert!(explain("lsuite AI error 401: bad token\nCheck the API key in Settings › Agent.").contains("Sign in again"));
        // The site's own words are kept when they say it all.
        let site = explain("lsuite AI error 402: Your lsuite AI allowance for this month is used up (Pro, 4,000 credits). It resets on 1 Nov. Manage plan: https://lsuite.xyz/account");
        assert!(site.contains("resets on 1 Nov") && site.matches("/account").count() == 1, "{site}");
        assert!(explain("lsuite AI error 403: lsuite AI needs a plan: your account is on Free.").ends_with("/account"));
    }
}
