#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use clap::Parser;
use tauri::{
    Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent,
    menu::{Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
};

mod runtime;

#[derive(Parser)]
#[command(name = "bitcraft", version, about = "Bitcraft desktop application")]
struct Args {
    #[arg(long, conflicts_with = "no_tray")]
    background: bool,
    #[arg(long)]
    no_tray: bool,
    #[arg(long)]
    home: Option<PathBuf>,
    #[arg(long, help = "Run the desktop shell without starting the game runtime")]
    no_runtime: bool,
}

struct Desktop {
    runtime: Mutex<Option<Arc<runtime::Runtime>>>,
    message: Mutex<String>,
    tray_available: AtomicBool,
    #[cfg(target_os = "linux")]
    background_pending: AtomicBool,
}

fn show_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
}

fn report(app: &tauri::AppHandle, message: &str) {
    *app.state::<Desktop>().message.lock().unwrap() = message.to_owned();
    if let Some(window) = app.get_webview_window("main") {
        let text = serde_json::to_string(message).unwrap();
        let _ = window.eval(format!(
            "document.getElementById('status').textContent = {text};"
        ));
    }
}

fn close_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }
}

fn run_menu_action(app: &tauri::AppHandle, id: &str) {
    match id {
        "open" => show_window(app),
        "close" => close_window(app),
        "quit" => app.exit(0),
        _ => {}
    }
}

fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open", "Open Bitcraft", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close", "Close Window", true, Some("CmdOrCtrl+W"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit Bitcraft", true, Some("CmdOrCtrl+Q"))?;
    Menu::with_items(app, &[&open, &close, &quit])
}

fn is_daemon_url(url: &tauri::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(runtime::HTTP_PORT)
}

/// Sites the game page may open in the user's browser.
const EXTERNAL_LINK_HOSTS: &[&str] = &["bitcraft.sh", "discord.gg"];

fn is_external_link(url: &tauri::Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url
            .host_str()
            .is_some_and(|host| EXTERNAL_LINK_HOSTS.contains(&host))
}

fn show_page_title(window: tauri::WebviewWindow, title: String) {
    let _ = window.set_title(&title);
}

fn allow_navigation(url: &tauri::Url) -> bool {
    is_daemon_url(url)
        || url.scheme() == "tauri"
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
}

fn launch_runtime(app: &tauri::AppHandle, home: PathBuf) -> Result<Arc<runtime::Runtime>> {
    let (resources, binary) = if cfg!(debug_assertions) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        (
            root.join("runtime"),
            root.join("binaries").join(format!(
                "bitcraft-dobjd-{}{}",
                env!("BITCRAFT_TARGET"),
                std::env::consts::EXE_SUFFIX
            )),
        )
    } else {
        (
            app.path().resource_dir()?.join("runtime"),
            std::env::current_exe()?
                .parent()
                .unwrap()
                .join(format!("bitcraft-dobjd{}", std::env::consts::EXE_SUFFIX)),
        )
    };
    Ok(Arc::new(runtime::Runtime::launch(
        &home, &resources, &binary,
    )?))
}

#[cfg(target_os = "linux")]
async fn tray_host_available() -> bool {
    let check = async {
        let connection = zbus::Connection::session().await.ok()?;
        let proxy = zbus::Proxy::new(
            &connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )
        .await
        .ok()?;
        proxy
            .get_property::<bool>("IsStatusNotifierHostRegistered")
            .await
            .ok()
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), check)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn watch_tray(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let available = tray_host_available().await;
            let state = app.state::<Desktop>();
            let was_available = state.tray_available.swap(available, Ordering::Relaxed);
            if available && state.background_pending.swap(false, Ordering::Relaxed) {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || hide_window(&handle));
            } else if was_available && !available {
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || show_window(&handle));
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

fn main() -> Result<()> {
    let args = Args::parse();
    let home = bitcraft_launchers::resolve_home(args.home)?;
    let desktop = Desktop {
        runtime: Mutex::new(None),
        message: Mutex::new(if args.no_runtime {
            "Desktop shell only. Start without --no-runtime to launch the game.".into()
        } else {
            "Preparing artifacts...".into()
        }),
        tray_available: AtomicBool::new(false),
        #[cfg(target_os = "linux")]
        background_pending: AtomicBool::new(args.background),
    };
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|arg| arg == "--background") {
                show_window(app);
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(desktop)
        .on_menu_event(|app, event| run_menu_action(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            // Other windows, such as Recipe Info, close normally.
            if window.label() == "main"
                && let WindowEvent::CloseRequested { api, .. } = event
                && window.state::<Desktop>().tray_available.load(Ordering::Relaxed)
            {
                api.prevent_close();
                hide_window(window.app_handle());
            }
        })
        .setup(move |app| {
            let new_window_app = app.handle().clone();
            let next_window_number = AtomicUsize::new(1);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Bitcraft")
                .on_document_title_changed(show_page_title)
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 480.0)
                .on_navigation(allow_navigation)
                // Built here rather than by the webview's default, so a window the
                // page opens keeps the same navigation limits and shows its page's
                // title.
                .on_new_window(move |url, features| {
                    if is_external_link(&url) {
                        if let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
                            eprintln!("Could not open {url} in the browser: {error}");
                        }
                        return tauri::webview::NewWindowResponse::Deny;
                    }
                    if !is_daemon_url(&url) {
                        return tauri::webview::NewWindowResponse::Deny;
                    }
                    let label = format!("page-{}", next_window_number.fetch_add(1, Ordering::Relaxed));
                    match WebviewWindowBuilder::new(&new_window_app, label, WebviewUrl::External(url))
                        .window_features(features)
                        .on_navigation(allow_navigation)
                        .on_document_title_changed(show_page_title)
                        .build()
                    {
                        Ok(window) => tauri::webview::NewWindowResponse::Create { window },
                        Err(error) => {
                            eprintln!("Could not open a window: {error}");
                            tauri::webview::NewWindowResponse::Deny
                        }
                    }
                })
                .on_page_load(|window, event| {
                    if matches!(event.event(), tauri::webview::PageLoadEvent::Finished) && window.url().is_ok_and(|url| url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost")) {
                        let message = window.state::<Desktop>().message.lock().unwrap().clone();
                        report(window.app_handle(), &message);
                    }
                })
                .initialization_script("document.addEventListener('DOMContentLoaded', () => { const style = document.createElement('style'); style.textContent = 'body { user-select: none; -webkit-user-select: none; } input, textarea, [contenteditable] { user-select: text; -webkit-user-select: text; }'; document.head.appendChild(style); });")
                .build()?;
            let app_menu = menu(app.handle())?;
            let submenu = Submenu::with_items(app, "Bitcraft", true, &app_menu.items()?.iter().map(|item| item as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect::<Vec<_>>())?;
            app.set_menu(Menu::with_items(app, &[&submenu])?)?;
            if !args.no_tray {
                match TrayIconBuilder::with_id("bitcraft")
                    .menu(&menu(app.handle())?)
                    .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                    .icon_as_template(cfg!(target_os = "macos"))
                    .tooltip("Bitcraft")
                    .build(app)
                {
                    Ok(_) => {
                        #[cfg(target_os = "linux")]
                        watch_tray(app.handle().clone());
                        #[cfg(not(target_os = "linux"))]
                        {
                            app.state::<Desktop>().tray_available.store(true, Ordering::Relaxed);
                            if args.background { hide_window(app.handle()); }
                        }
                    }
                    Err(error) => eprintln!("System tray unavailable: {error}"),
                }
            }
            if !args.no_runtime {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let launch_handle = handle.clone();
                    let runtime = tauri::async_runtime::spawn_blocking(move || launch_runtime(&launch_handle, home))
                        .await.map_err(anyhow::Error::from).and_then(|result| result);
                    match runtime {
                        Ok(runtime) => {
                            *handle.state::<Desktop>().runtime.lock().unwrap() = Some(runtime.clone());
                            match runtime.wait_until_ready().await {
                                Ok(()) => {
                                    if let Some(window) = handle.get_webview_window("main") {
                                        let _ = window.navigate(format!("http://127.0.0.1:{}/bitcraft/", runtime::HTTP_PORT).parse().unwrap());
                                    }
                                }
                                Err(error) => {
                                    let _ = runtime.stop();
                                    report(&handle, &format!("Bitcraft could not start.\n\n{error:#}"));
                                    show_window(&handle);
                                }
                            }
                        }
                        Err(error) => {
                            report(&handle, &format!("Bitcraft could not start.\n\n{error:#}"));
                            show_window(&handle);
                        }
                    }
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())?;
    app.run(|app, event| match event {
        RunEvent::Exit => {
            if let Some(runtime) = app.state::<Desktop>().runtime.lock().unwrap().take() {
                let _ = runtime.stop();
            }
        }
        // A Finder or Spotlight launch of the running app arrives here, not as a second instance.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_window(app),
        _ => {}
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_stays_on_the_app_or_its_daemon() {
        for url in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "http://127.0.0.1:7727/bitcraft/",
        ] {
            assert!(allow_navigation(&url.parse().unwrap()), "{url}");
        }
        for url in [
            "http://127.0.0.1:7717/",
            "https://example.com",
            "file:///etc/passwd",
            "http://127.0.0.1.example.com:7727/",
        ] {
            assert!(!allow_navigation(&url.parse().unwrap()), "{url}");
        }
        assert!(is_daemon_url(
            &"http://127.0.0.1:7727/objects/a.dobj/raw".parse().unwrap()
        ));
        assert!(!is_daemon_url(
            &"tauri://localhost/index.html".parse().unwrap()
        ));
    }

    #[test]
    fn only_listed_sites_open_in_the_browser() {
        for url in [
            "https://bitcraft.sh/",
            "https://bitcraft.sh/guide?x=1",
            "https://discord.gg/cWtZZYrer",
        ] {
            assert!(is_external_link(&url.parse().unwrap()), "{url}");
        }
        for url in [
            "http://bitcraft.sh/",
            "https://bitcraft.sh:8443/",
            "https://bitcraft.sh.example.com/",
            "https://bitcraft.digiobjects.net/",
            "https://example.com/",
            "file:///etc/passwd",
        ] {
            assert!(!is_external_link(&url.parse().unwrap()), "{url}");
        }
    }
}
