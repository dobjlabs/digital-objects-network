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
use tokio::sync::watch;

mod agents;
mod discord;
mod metrics;
mod onboarding;
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
    home: PathBuf,
    discord: discord::Presence,
    inventory_metrics: watch::Sender<Option<metrics::InventoryMetrics>>,
    metrics_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    runtime: Mutex<Option<Arc<runtime::Runtime>>>,
    message: Mutex<String>,
    tray_available: AtomicBool,
    #[cfg(target_os = "linux")]
    background_pending: AtomicBool,
}

impl Desktop {
    fn new(home: PathBuf, message: String) -> Self {
        Self {
            discord: discord::Presence::new(&home),
            home,
            inventory_metrics: watch::Sender::new(None),
            metrics_task: Mutex::new(None),
            runtime: Mutex::new(None),
            message: Mutex::new(message),
            tray_available: AtomicBool::new(false),
            #[cfg(target_os = "linux")]
            background_pending: AtomicBool::new(false),
        }
    }
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
    let windows = app.webview_windows();
    let focused = windows
        .values()
        .find(|window| window.is_focused().unwrap_or(false));
    if let Some(window) = focused.cloned().or_else(|| app.get_webview_window("main")) {
        let _ = window.close();
    }
}

fn run_menu_action(app: &tauri::AppHandle, id: &str) {
    match id {
        "open" => show_window(app),
        "close" => close_window(app),
        "agent-guide" => {
            if let Err(error) = onboarding::show(app, None) {
                eprintln!("Could not open agent guide: {error:#}");
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, "open", "Open Bitcraft", true, None::<&str>)?;
    let close = MenuItem::with_id(app, "close", "Close Window", true, Some("CmdOrCtrl+W"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit Bitcraft", true, Some("CmdOrCtrl+Q"))?;
    let agents = MenuItem::with_id(
        app,
        "agent-guide",
        "Play with agents...",
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&open, &agents, &close, &quit])
}

/// The editing commands, whose items carry the shortcuts that let pages cut,
/// copy, paste and select text on macOS.
#[cfg(target_os = "macos")]
fn edit_menu(app: &tauri::AppHandle) -> tauri::Result<Submenu<tauri::Wry>> {
    use tauri::menu::PredefinedMenuItem;
    Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )
}

fn shows_page(url: &tauri::Url, page: &str) -> bool {
    is_daemon_url(url)
        && url
            .query_pairs()
            .any(|(key, value)| key == "page" && value == page)
}

/// Brings the window showing `page` to the front, which the page's own
/// `focus()` on that window does not.
#[tauri::command]
fn raise_window(app: tauri::AppHandle, page: String) {
    let window = app
        .webview_windows()
        .into_values()
        .find(|window| window.url().is_ok_and(|url| shows_page(&url, &page)));
    if let Some(window) = window {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn is_daemon_url(url: &tauri::Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(runtime::HTTP_PORT)
}

/// Sites the game page may open in the user's browser.
const EXTERNAL_LINK_HOSTS: &[&str] = &["bitcraft.sh", "discord.gg"];

fn open_in_browser(url: &tauri::Url) {
    if let Err(error) = tauri_plugin_opener::open_url(url.as_str(), None::<&str>) {
        eprintln!("Could not open {url} in the browser: {error}");
    }
}

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

#[tauri::command]
fn inventory_metrics(desktop: tauri::State<Desktop>) -> Option<metrics::InventoryMetrics> {
    desktop.inventory_metrics.borrow().clone()
}

#[tauri::command]
fn discord_sharing(desktop: tauri::State<Desktop>) -> bool {
    desktop.discord.enabled()
}

/// Returns the preference now in effect, which is the requested one even
/// when saving it fails.
#[tauri::command]
fn set_discord_sharing(desktop: tauri::State<Desktop>, enabled: bool) -> bool {
    if let Err(error) = desktop.discord.set_enabled(enabled) {
        eprintln!("Could not save Discord sharing preference: {error}");
    }
    desktop.discord.enabled()
}

fn shell_dev_url(app: &tauri::AppHandle) -> Option<tauri::Url> {
    if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    }
}

fn is_local_app_url(url: &tauri::Url, dev_url: Option<&tauri::Url>) -> bool {
    url.scheme() == "tauri"
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
        || dev_url.is_some_and(|dev_url| {
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.origin() == dev_url.origin()
        })
}

fn allow_navigation(url: &tauri::Url, dev_url: Option<&tauri::Url>) -> bool {
    is_daemon_url(url) || is_local_app_url(url, dev_url)
}

fn runtime_paths(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf)> {
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
    Ok((resources, binary))
}

fn launch_runtime(app: &tauri::AppHandle, home: PathBuf) -> Result<Arc<runtime::Runtime>> {
    let (resources, binary) = runtime_paths(app)?;
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
    let desktop = Desktop::new(
        home.clone(),
        if args.no_runtime {
            "Desktop shell only. Start without --no-runtime to launch the game.".into()
        } else {
            "Preparing artifacts...".into()
        },
    );
    #[cfg(target_os = "linux")]
    desktop
        .background_pending
        .store(args.background, Ordering::Relaxed);
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            inventory_metrics,
            discord_sharing,
            set_discord_sharing,
            raise_window
        ])
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
            let dev_url = shell_dev_url(app.handle());
            let main_dev_url = dev_url.clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Bitcraft")
                .on_document_title_changed(show_page_title)
                .inner_size(1100.0, 760.0)
                .min_inner_size(720.0, 480.0)
                .on_navigation(move |url| allow_navigation(url, main_dev_url.as_ref()))
                // Built here rather than by the webview's default, so a window the
                // page opens keeps the same navigation limits and shows its page's
                // title.
                .on_new_window(move |url, features| {
                    if let Some(agent) = agents::Agent::from_request(&url) {
                        agents::request(&new_window_app, agent);
                        return tauri::webview::NewWindowResponse::Deny;
                    }
                    if is_external_link(&url) {
                        open_in_browser(&url);
                        return tauri::webview::NewWindowResponse::Deny;
                    }
                    if !is_daemon_url(&url) {
                        return tauri::webview::NewWindowResponse::Deny;
                    }
                    let label = format!("page-{}", next_window_number.fetch_add(1, Ordering::Relaxed));
                    let dev_url = dev_url.clone();
                    match WebviewWindowBuilder::new(&new_window_app, label, WebviewUrl::External(url))
                        .window_features(features)
                        .on_navigation(move |url| allow_navigation(url, dev_url.as_ref()))
                        .on_document_title_changed(show_page_title)
                        // Such a window opens only listed sites, in the browser.
                        .on_new_window(|url, _| {
                            if is_external_link(&url) {
                                open_in_browser(&url);
                            }
                            tauri::webview::NewWindowResponse::Deny
                        })
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
                    if matches!(event.event(), tauri::webview::PageLoadEvent::Finished) && window.url().is_ok_and(|url| is_local_app_url(&url, shell_dev_url(window.app_handle()).as_ref())) {
                        let message = window.state::<Desktop>().message.lock().unwrap().clone();
                        report(window.app_handle(), &message);
                    }
                })
                .initialization_script("window.bitcraftDesktop = true; document.addEventListener('DOMContentLoaded', () => { const style = document.createElement('style'); style.textContent = 'body { user-select: none; -webkit-user-select: none; } input, textarea, [contenteditable] { user-select: text; -webkit-user-select: text; }'; document.head.appendChild(style); });")
                .build()?;
            let app_menu = menu(app.handle())?;
            let submenu = Submenu::with_items(app, "Bitcraft", true, &app_menu.items()?.iter().map(|item| item as &dyn tauri::menu::IsMenuItem<tauri::Wry>).collect::<Vec<_>>())?;
            let menu_bar = Menu::with_items(app, &[&submenu])?;
            #[cfg(target_os = "macos")]
            menu_bar.append(&edit_menu(app.handle())?)?;
            app.set_menu(menu_bar)?;
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
                                    let desktop = handle.state::<Desktop>();
                                    *desktop.metrics_task.lock().unwrap() = Some(tauri::async_runtime::spawn(metrics::run(runtime.connection(), desktop.inventory_metrics.clone())));
                                    desktop.discord.start(runtime.connection(), desktop.inventory_metrics.subscribe());
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
            if let Some(task) = app.state::<Desktop>().metrics_task.lock().unwrap().take() {
                task.abort();
            }
            app.state::<Desktop>().discord.stop();
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
    fn native_capabilities_use_the_runtime_port() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        for identifier in ["main-page", "discord-sharing"] {
            let capability = config["app"]["security"]["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .find(|capability| capability["identifier"] == identifier)
                .unwrap_or_else(|| panic!("{identifier} capability must exist"));
            assert_eq!(
                capability["remote"]["urls"],
                serde_json::json!([format!("http://127.0.0.1:{}/*", runtime::HTTP_PORT)]),
                "{identifier}"
            );
        }
    }

    #[test]
    fn settings_window_calls_are_allowed_under_the_main_window_label() {
        // On macOS a window the page opens shares the main window's webview
        // configuration, so its IPC calls arrive labelled as the main window's.
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let capability = config["app"]["security"]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|capability| capability["identifier"] == "discord-sharing")
            .unwrap();
        assert_eq!(capability["windows"], serde_json::json!(["main", "page-*"]));
    }

    #[test]
    fn navigation_stays_on_the_app_or_its_daemon() {
        for url in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "http://127.0.0.1:7727/bitcraft/",
        ] {
            assert!(allow_navigation(&url.parse().unwrap(), None), "{url}");
        }
        for url in [
            "http://127.0.0.1:7717/",
            "https://example.com",
            "file:///etc/passwd",
            "http://127.0.0.1.example.com:7727/",
        ] {
            assert!(!allow_navigation(&url.parse().unwrap(), None), "{url}");
        }
        assert!(is_daemon_url(
            &"http://127.0.0.1:7727/objects/a.dobj/raw".parse().unwrap()
        ));
        assert!(!is_daemon_url(
            &"tauri://localhost/index.html".parse().unwrap()
        ));
    }

    #[test]
    fn a_page_matches_only_the_daemon_window_showing_it() {
        let window = |url: &str| -> tauri::Url { url.parse().unwrap() };
        assert!(shows_page(
            &window("http://127.0.0.1:7727/bitcraft/?page=summary"),
            "summary"
        ));
        assert!(shows_page(
            &window("http://127.0.0.1:7727/bitcraft/?page=recipebook&class=bitcraft::Wood"),
            "recipebook"
        ));
        assert!(!shows_page(
            &window("http://127.0.0.1:7727/bitcraft/?page=settings"),
            "summary"
        ));
        assert!(!shows_page(
            &window("http://127.0.0.1:7727/bitcraft/"),
            "summary"
        ));
        assert!(!shows_page(
            &window("https://example.com/?page=summary"),
            "summary"
        ));
    }

    #[test]
    fn shell_navigation_accepts_only_the_configured_development_origin() {
        let dev_url = "http://127.0.0.1:1430/".parse().unwrap();
        for url in ["http://127.0.0.1:1430/", "http://127.0.0.1:1430/index.html"] {
            let url = url.parse().unwrap();
            assert!(!is_local_app_url(&url, None));
            assert!(is_local_app_url(&url, Some(&dev_url)));
            assert!(allow_navigation(&url, Some(&dev_url)));
        }
        for url in [
            "http://127.0.0.1:1431/index.html",
            "https://127.0.0.1:1430/index.html",
            "http://127.0.0.1.example.com:1430/index.html",
            "http://user@127.0.0.1:1430/index.html",
            "http://127.0.0.1:7727/bitcraft/",
            "https://example.com/index.html",
        ] {
            assert!(
                !is_local_app_url(&url.parse().unwrap(), Some(&dev_url)),
                "{url}"
            );
        }
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
