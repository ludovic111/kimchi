//! `account.*`: the lsuite account shared by every lsuite app on this computer ([`crate::account`]),
//! which runs the agent on lsuite AI without any other setup. Signing in and out stays with the
//! person (agents can read the status and the plans).

use std::sync::Arc;

use serde_json::{Value, json};

use crate::account;
use crate::registry::{Args, Ctx};
use crate::session::{CmdResult, Event, Session, ToastKind};

pub async fn run(s: &Arc<Session>, cx: &Ctx, a: Args) -> CmdResult {
    match cx.spec.name {
        "account.status" => Ok(status_json(&account::status(a.bool_or("offline", false)).await)),
        "account.plans" => account::plans().await,
        "account.signIn" => {
            if a.bool_or("cancel", false) {
                return Ok(json!({ "cancelled": account::cancel_sign_in() }));
            }
            if let Some(key) = a.opt_str("key") {
                let acc = account::sign_in_with_key(key).await?;
                signed_in(s, &acc);
                return Ok(json!({ "signedIn": true, "account": account::public(&acc) }));
            }
            let pending = account::begin_sign_in().await?;
            let url = pending.url.clone();
            let opened = account::open_browser(&url);
            let open_line = if opened { "Finish signing in in your browser." } else { "Open this address in a browser to sign in." };
            if !a.bool_or("wait", true) {
                let session = s.clone();
                s.runtime().spawn(async move {
                    match pending.finish(account::SIGN_IN_TIMEOUT).await {
                        Ok(acc) => signed_in(&session, &acc),
                        Err(e) if e.contains("cancelled") => session.emit(Event::SettingsChanged),
                        Err(e) => {
                            session.toast(ToastKind::Error, e);
                            session.emit(Event::SettingsChanged);
                        }
                    }
                });
                s.emit(Event::SettingsChanged);
                return Ok(json!({ "signedIn": false, "waiting": true, "url": url, "opened": opened, "message": open_line }));
            }
            if !opened && std::env::var("KIMCHI_NO_BROWSER").is_err() {
                account::cancel_sign_in();
                return Err(format!("There is no browser to open here. Sign in at {} and paste the key it shows: account.signIn key=lsk_…", account::manage_url(&account::server())));
            }
            let acc = pending.finish(account::SIGN_IN_TIMEOUT).await?;
            signed_in(s, &acc);
            Ok(json!({ "signedIn": true, "url": url, "account": account::public(&acc) }))
        }
        "account.signOut" => {
            let was = account::sign_out().await?;
            s.emit(Event::SettingsChanged);
            Ok(json!({ "signedOut": was }))
        }
        _ => Err(super::unhandled(cx)),
    }
}

fn signed_in(s: &Arc<Session>, acc: &account::Account) {
    let who = if acc.email.is_empty() { "your lsuite account".to_string() } else { acc.email.clone() };
    s.toast(ToastKind::Success, format!("Signed in to lsuite AI as {who}. Your other lsuite apps are signed in too."));
    s.emit(Event::SettingsChanged);
}

/// The status as commands answer it.
pub fn status_json(st: &account::Status) -> Value {
    let mut v = json!(st);
    v["signingIn"] = json!(account::signing_in());
    v
}
