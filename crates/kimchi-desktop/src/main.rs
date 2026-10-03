//! kimchi: the desktop window (GPUI) around the Rust editing core.
//!
//! The window is one client of the command registry in `kimchi-control`, like
//! the built-in agent, `kimchi-cli` and `kimchi-mcp`; it starts the session,
//! the loopback bridge those tools use, and writes the lsuite discovery file.

// No console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod assets;
mod playback;
mod preview;
mod store;
mod theme;
mod ui;
mod views;

#[cfg(test)]
mod tests;

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, px, size};
use kimchi_control::{Session, SessionOptions};

fn main() {
    // Logs to `<data>/logs/kimchi.log` (and stderr), crash reports, and the note of how the last
    // run ended: first, so everything after is recorded.
    let data_dir = kimchi_control::session::default_data_dir();
    let config_dir = kimchi_control::session::default_config_dir();
    let level = kimchi_control::Settings::load(&config_dir).diagnostics.log_level;
    let started = kimchi_control::diagnostics::init(&data_dir, "kimchi", &level, true);

    // Background work (ffmpeg, providers, the bridge) runs on Tokio; GPUI drives the window.
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("kimchi-worker").build().expect("tokio runtime");
    let session = {
        let _guard = runtime.enter();
        Session::new(SessionOptions {
            secrets: Some(kimchi_control::secrets::default_store()),
            headless: false,
            ..Default::default()
        })
        .unwrap_or_else(|e| fatal(&format!("kimchi couldn't create its folders in {} or {}: {e}", data_dir.display(), config_dir.display())))
    };

    // The bridge for kimchi-cli and kimchi-mcp, and the lsuite discovery entry.
    let bridge = runtime.block_on(kimchi_control::bridge::Server::start(session.clone()));
    let running = match &bridge {
        Ok(server) => Some(kimchi_control::discovery::Running {
            pid: std::process::id(),
            control_file: Some(server.path().to_path_buf()),
            port: Some(server.port()),
            since: chrono::Utc::now(),
        }),
        Err(e) => {
            tracing::warn!("the control bridge couldn't start: {e}");
            None
        }
    };
    if let Err(e) = kimchi_control::discovery::write(&kimchi_control::discovery::entry(&session.data_dir, running)) {
        tracing::warn!("couldn't write ~/.lsuite/apps/kimchi.json: {e}");
    }

    // Logging out or shutting down sends SIGTERM: that is a proper end (every change is
    // already saved), not one to report next time.
    #[cfg(unix)]
    {
        let data_dir = session.data_dir.clone();
        runtime.spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            let (Ok(mut term), Ok(mut hup), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::hangup()), signal(SignalKind::interrupt())) else {
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = hup.recv() => {}
                _ = int.recv() => {}
            }
            tracing::info!("asked to stop by the system");
            let _ = kimchi_control::discovery::write(&kimchi_control::discovery::entry(&data_dir, None));
            kimchi_control::diagnostics::clean_exit();
            std::process::exit(0);
        });
    }

    let handle = runtime.handle().clone();
    // GPUI's run loop doesn't return on macOS: cleanup happens when the app quits.
    let bridge = parking_lot::Mutex::new(bridge.ok());
    gpui_platform::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        gpui_tokio::init_from_handle(cx, handle);
        assets::load_fonts(cx);
        app::init(session.clone(), cx);
        open_main_window(cx);
        cx.activate(true);
        // This copy started: an update's previous copy can go.
        kimchi_control::update::finish_pending();
        welcome(&session, &started, cx);

        // Check for updates a few seconds after start and every few hours (unless turned off).
        let s = session.clone();
        gpui_tokio::Tokio::spawn(cx, kimchi_control::update::run_in_background(s)).detach();

        // On quit: say so in the discovery file and stop the bridge (removes control.json); a
        // downloaded Windows installer runs once kimchi has exited.
        let s = session.clone();
        cx.on_app_quit(move |_| {
            let _ = kimchi_control::discovery::write(&kimchi_control::discovery::entry(&s.data_dir, None));
            drop(bridge.lock().take());
            kimchi_control::update::apply_on_quit();
            kimchi_control::diagnostics::clean_exit();
            async {}
        })
        .detach();
    });
    kimchi_control::diagnostics::clean_exit();
    drop(runtime);
}

/// What the window says once at start: what's new after an update, and that the last run
/// ended badly (with its report a click away).
fn welcome(session: &std::sync::Arc<Session>, started: &kimchi_control::diagnostics::Started, cx: &mut App) {
    use crate::store::{Dialog, StoreExt};
    let previous = kimchi_control::release_notes::take_unseen(&session.config_dir);
    let reports = kimchi_control::diagnostics::unseen_reports(&session.data_dir, &session.config_dir);
    let show_notes = session.settings().updates.show_whats_new && std::env::var("KIMCHI_NO_WHATS_NEW").is_err();
    let store = cx.store();
    store.update(cx, |s, cx| {
        if let Some(since) = previous.filter(|_| show_notes) {
            s.open_dialog(Dialog::WhatsNew { since: Some(since), all: false }, cx);
        }
        if !reports.is_empty() {
            let unclean = !started.unclean.is_empty() && reports.iter().all(|r| r.kind == "unclean");
            let text = if unclean { "kimchi didn't quit properly last time. Its log was kept." } else { "kimchi ran into a problem last time and wrote a crash report." };
            s.toast_with(kimchi_control::ToastKind::Error, text, "View", Dialog::Settings { section: Some("diagnostics".into()) }, cx);
        }
    });
}

/// Before the window exists: log it, say it on stderr and stop.
fn fatal(message: &str) -> ! {
    tracing::error!("{message}");
    eprintln!("{message}");
    std::process::exit(1)
}

pub fn open_main_window(cx: &mut App) {
    // `KIMCHI_WINDOW_SIZE=2000x1250` opens the window at that size (screenshots, tests).
    let (w, h) = std::env::var("KIMCHI_WINDOW_SIZE")
        .ok()
        .and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))))
        .unwrap_or((1480., 920.));
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let transparent = cx.global::<theme::Theme>().transparent;
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // kimchi's top bar is the title bar: the traffic lights sit in it on macOS, and
        // `ui::window_controls` draws the buttons on Windows and client-decorated Linux.
        titlebar: Some(TitlebarOptions { title: Some("kimchi".into()), appears_transparent: true, traffic_light_position: Some(point(px(16.), px(17.))) }),
        focus: true,
        show: true,
        window_min_size: Some(size(px(1100.), px(680.))),
        // The window material on macOS; the CSS-like tiers sit on top of it.
        window_background: if transparent { WindowBackgroundAppearance::Blurred } else { WindowBackgroundAppearance::Opaque },
        app_id: Some("kimchi".into()),
        icon: image::load_from_memory(include_bytes!("../resources/kimchi.png")).ok().map(|i| std::sync::Arc::new(i.to_rgba8())),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| cx.new(|cx| app::Workspace::new(window, cx))).expect("couldn't open the kimchi window");
}
