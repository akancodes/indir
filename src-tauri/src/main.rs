// No console window beside the app on Windows, in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod downloads;
mod settings;
mod store;
mod update;

use downloads::{Downloads, Location, View};
use settings::Settings;
use tauri::{AppHandle, Manager, RunEvent, State, Url, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

const PROJECT: &str = "https://github.com/akancodes/indir";

fn main() {
    let app = tauri::Builder::default()
        // A second launch brings the first window forward instead of running twice.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| bring_forward(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(Downloads::load(app.handle())?);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
                .title("indir")
                .inner_size(820.0, 560.0)
                .min_inner_size(480.0, 360.0)
                .center()
                // The page takes links dropped on it…
                .disable_drag_drop_handler()
                // …and never opens one itself.
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
            remove,
            clear_finished,
            open_file,
            show_in_folder,
            get_settings,
            set_connections,
            choose_folder,
            version,
            check_update,
            open_releases,
            open_project,
        ])
        .build(tauri::generate_context!())
        .expect("indir couldn't start");
    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Running downloads stop with the app; what reached the disk is kept.
            app.state::<Downloads>().save();
        }
    });
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

fn bring_forward(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
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
fn remove(downloads: State<Downloads>, id: u64) {
    downloads.remove(id);
}

#[tauri::command]
fn clear_finished(downloads: State<Downloads>) {
    downloads.clear_finished();
}

#[tauri::command]
fn open_file(app: AppHandle, downloads: State<Downloads>, id: u64) -> Result<(), String> {
    let path = downloads.finished_path(id)?;
    app.opener().open_path(path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn show_in_folder(app: AppHandle, downloads: State<Downloads>, id: u64) -> Result<(), String> {
    match downloads.location(id)? {
        Location::File(path) => app.opener().reveal_item_in_dir(path),
        Location::Folder(folder) => app.opener().open_path(folder.to_string_lossy(), None::<&str>),
    }
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_settings(downloads: State<Downloads>) -> Settings {
    downloads.settings()
}

#[tauri::command]
fn set_connections(downloads: State<Downloads>, connections: u8) -> Settings {
    downloads.set_connections(connections)
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
