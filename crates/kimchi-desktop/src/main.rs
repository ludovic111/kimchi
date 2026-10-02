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

use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, point, px, size};
use kimchi_control::{Session, SessionOptions};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,kimchi=debug".into()))
        .with_writer(std::io::stderr)
        .init();

    // Background work (ffmpeg, providers, the bridge) runs on Tokio; GPUI drives the window.
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("kimchi-worker").build().expect("tokio runtime");
    let session = {
        let _guard = runtime.enter();
        Session::new(SessionOptions {
            secrets: Some(kimchi_control::secrets::default_store()),
            headless: false,
            ..Default::default()
        })
        .expect("couldn't create kimchi's data folders")
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

        // Check for updates a few seconds after start (unless turned off).
        let s = session.clone();
        gpui_tokio::Tokio::spawn(cx, async move {
            tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            kimchi_control::update::check_on_start(&s).await;
        })
        .detach();

        // On quit: say so in the discovery file and stop the bridge (removes control.json).
        let s = session.clone();
        cx.on_app_quit(move |_| {
            let _ = kimchi_control::discovery::write(&kimchi_control::discovery::entry(&s.data_dir, None));
            drop(bridge.lock().take());
            async {}
        })
        .detach();
    });
    drop(runtime);
}

pub fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1480.), px(920.)), cx);
    let transparent = cx.global::<theme::Theme>().transparent;
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
