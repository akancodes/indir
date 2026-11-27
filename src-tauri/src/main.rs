// No console window beside the app on Windows, in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod downloads;
mod power;
mod settings;
mod store;
mod update;

use clipboard::Clipboard;
use downloads::{Downloads, Location, View, WhenDone};
use settings::Settings;
use tauri::{AppHandle, Manager, RunEvent, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

const PROJECT: &str = "https://github.com/akancodes/indir";

fn main() {
    let app = tauri::Builder::default()
        // A second launch brings the first window forward instead of running twice.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| bring_forward(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // Each window opens where it was last, at the size it was, unless that place is off every screen.
        .plugin(tauri_plugin_window_state::Builder::default().with_state_flags(window_state()).build())
        .setup(|app| {
            app.manage(Downloads::load(app.handle())?);
            app.manage(Clipboard::default());
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("indir")
                .inner_size(1000.0, 680.0)
                .min_inner_size(480.0, 360.0)
                .center()
                // The page takes links dropped on it…
                .disable_drag_drop_handler()
                // …and never opens one itself.
                .on_navigation(is_own_page)
                .build()?;
            // Mini mode: a small strip that stays on top, hidden until it's asked for.
            WebviewWindowBuilder::new(app, "mini", WebviewUrl::App("mini.html".into()))
                .title("indir")
                .inner_size(320.0, 90.0)
                .resizable(false)
                .maximizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .disable_drag_drop_handler()
                .on_navigation(is_own_page)
                .build()?;
            downloads::watch(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list,
            add,
            pause,
            resume,
            start_now,
            remove,
            clear_finished,
            pause_all,
            resume_all,
            open_file,
            show_in_folder,
            copy_link,
            clipboard_links,
            get_when_done,
            set_when_done,
            cancel_when_done,
            when_done_now,
            get_settings,
            set_connections,
            set_at_once,
            set_speed_limit,
            choose_folder,
            version,
            check_update,
            open_releases,
            open_project,
            show_mini,
            show_main,
            drag_mini,
        ])
        .on_window_event(|window, event| match (window.label(), event) {
            // The strip never closes on its own: closing it goes back to the main window.
            ("mini", WindowEvent::CloseRequested { api, .. }) => {
                api.prevent_close();
                bring_forward(window.app_handle());
            }
            // The hidden strip would keep indir running once the main window is gone.
            ("main", WindowEvent::Destroyed) => window.app_handle().exit(0),
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("indir couldn't start");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Running downloads stop with the app; what reached the disk is kept.
            app.state::<Downloads>().save();
            let _ = app.save_window_state(window_state());
        }
    });
}

/// What is remembered of each window between launches.
fn window_state() -> StateFlags {
    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED
}

/// The window only ever shows indir's own page.
fn is_own_page(url: &Url) -> bool {
    match url.scheme() {
        "tauri" | "about" => true,
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || (cfg!(debug_assertions) && matches!(url.host_str(), Some("localhost" | "127.0.0.1")))
        }
        _ => false,
    }
}

/// Opens a finished file with the app the system picks for it.
fn open(app: &AppHandle, id: u64) -> Result<(), String> {
    let path = app.state::<Downloads>().finished_path(id)?;
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

/// Shows a download in its folder: the file, or the folder it goes to.
fn reveal(app: &AppHandle, id: u64) -> Result<(), String> {
    match app.state::<Downloads>().location(id)? {
        Location::File(path) => app.opener().reveal_item_in_dir(path),
        Location::Folder(folder) => app.opener().open_path(folder.to_string_lossy(), None::<&str>),
    }
    .map_err(|e| e.to_string())
}

/// Shows the main window, in front, in place of the strip.
pub fn bring_forward(app: &AppHandle) {
    if let Some(mini) = app.get_webview_window("mini") {
        let _ = mini.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Swaps the main window for the strip, unless the countdown is running: that stays in view.
#[tauri::command]
fn show_mini(app: AppHandle, downloads: State<Downloads>) {
    if downloads.counting_down() {
        bring_forward(&app);
        return;
    }
    let (Some(main), Some(mini)) = (app.get_webview_window("main"), app.get_webview_window("mini")) else { return };
    // Some window managers place a window afresh each time it shows; the strip goes back where it was.
    let place = mini.outer_position().ok();
    let _ = mini.show();
    if let Some(place) = place {
        let _ = mini.set_position(place);
    }
    let _ = mini.set_always_on_top(true);
    let _ = main.hide();
    let _ = mini.set_focus();
}

#[tauri::command]
fn show_main(app: AppHandle) {
    bring_forward(&app);
}

/// Moves the strip with the pointer, while its button is held.
#[tauri::command]
fn drag_mini(window: WebviewWindow) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
fn list(downloads: State<Downloads>) -> Vec<View> {
    downloads.views()
}

/// Adds every web link in the pasted text.
#[tauri::command]
fn add(app: AppHandle, downloads: State<Downloads>, text: String) -> Result<usize, String> {
    downloads.add(&app, &text)
}

#[tauri::command]
fn pause(downloads: State<Downloads>, id: u64) {
    downloads.pause(id);
}

#[tauri::command]
fn resume(app: AppHandle, downloads: State<Downloads>, id: u64) {
    downloads.resume(&app, id);
}

#[tauri::command]
fn start_now(app: AppHandle, downloads: State<Downloads>, id: u64) {
    downloads.start_now(&app, id);
}

#[tauri::command]
fn remove(downloads: State<Downloads>, id: u64) {
    downloads.remove(id);
}

#[tauri::command]
fn clear_finished(downloads: State<Downloads>) {
    downloads.clear_finished();
}

#[tauri::command]
fn pause_all(downloads: State<Downloads>) {
    downloads.pause_all();
}

#[tauri::command]
fn resume_all(app: AppHandle, downloads: State<Downloads>) {
    downloads.resume_all(&app);
}

#[tauri::command]
fn open_file(app: AppHandle, id: u64) -> Result<(), String> {
    open(&app, id)
}

#[tauri::command]
fn show_in_folder(app: AppHandle, id: u64) -> Result<(), String> {
    reveal(&app, id)
}

/// Puts a download's link on the clipboard.
#[tauri::command]
async fn copy_link(app: AppHandle, id: u64) -> Result<(), String> {
    let link = app.state::<Downloads>().link(id).ok_or("That download isn't on the list any more.")?;
    // Never on the main thread: on Linux the clipboard may wait on this very app.
    tauri::async_runtime::spawn_blocking(move || app.state::<Clipboard>().write(link))
        .await
        .map_err(|e| e.to_string())?
}

/// Links just copied, for the link field, if they are new.
#[tauri::command]
async fn clipboard_links(app: AppHandle) -> Option<String> {
    // Never on the main thread: on Linux the clipboard may wait on this very app.
    tauri::async_runtime::spawn_blocking(move || {
        let text = app.state::<Clipboard>().read().ok()?;
        app.state::<Downloads>().offer(&text)
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
fn get_when_done(downloads: State<Downloads>) -> WhenDone {
    downloads.when_done()
}

#[tauri::command]
fn set_when_done(app: AppHandle, downloads: State<Downloads>, action: WhenDone) {
    downloads.set_when_done(&app, action);
}

#[tauri::command]
fn cancel_when_done(app: AppHandle, downloads: State<Downloads>) {
    downloads.cancel_when_done(&app);
}

#[tauri::command]
fn when_done_now(app: AppHandle, downloads: State<Downloads>) {
    downloads.when_done_now(&app);
}

#[tauri::command]
fn get_settings(downloads: State<Downloads>) -> Settings {
    downloads.settings()
}

#[tauri::command]
fn set_connections(downloads: State<Downloads>, connections: u8) -> Settings {
    downloads.set_connections(connections)
}

#[tauri::command]
fn set_at_once(app: AppHandle, downloads: State<Downloads>, at_once: Option<u8>) -> Settings {
    downloads.set_at_once(&app, at_once)
}

/// In bytes a second, for every download together; none lifts the limit.
#[tauri::command]
fn set_speed_limit(downloads: State<Downloads>, limit: Option<u64>) -> Settings {
    downloads.set_speed_limit(limit)
}

/// Asks for a new download folder. Nothing changes if the dialog is cancelled.
#[tauri::command]
async fn choose_folder(app: AppHandle) -> Option<Settings> {
    let current = app.state::<Downloads>().settings().folder;
    let (chosen, picked) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Choose where downloads go").set_directory(current);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_folder(move |folder| {
        let _ = chosen.send(folder);
    });
    let folder = picked.await.ok()??.into_path().ok()?;
    Some(app.state::<Downloads>().set_folder(folder))
}

#[tauri::command]
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
async fn check_update(app: AppHandle) -> Option<update::Update> {
    let client = app.state::<Downloads>().client();
    update::check(&client).await
}

#[tauri::command]
fn open_releases(app: AppHandle) {
    let _ = app.opener().open_url(format!("{PROJECT}/releases/latest"), None::<&str>);
}

#[tauri::command]
fn open_project(app: AppHandle) {
    let _ = app.opener().open_url(PROJECT, None::<&str>);
}
