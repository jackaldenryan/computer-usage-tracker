mod tracker;

use chrono::Local;
use std::path::PathBuf;
use tauri::path::BaseDirectory;
use tauri::Manager;

fn settings_path(app: &tauri::AppHandle) -> PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    std::fs::create_dir_all(&dir).ok();
    dir.join("settings.json")
}

fn load_settings(app: &tauri::AppHandle) -> tracker::Settings {
    tracker::load_settings_from(&settings_path(app))
}

fn save_settings(app: &tauri::AppHandle, st: &tracker::Settings) {
    let _ = tracker::save_settings_to(&settings_path(app), st);
}

fn epoch_of(st: &tracker::Settings) -> Option<&str> {
    st.background_epoch.as_deref()
}

#[tauri::command]
fn get_entries(app: tauri::AppHandle) -> Result<Vec<tracker::DayEntry>, String> {
    let st = load_settings(&app);
    tracker::backfill(&st.csv_path, st.use_auto_subtract, epoch_of(&st))
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> tracker::Settings {
    load_settings(&app)
}

#[tauri::command]
fn get_today(app: tauri::AppHandle) -> Result<tracker::TodayInfo, String> {
    let st = load_settings(&app);
    let entries = tracker::backfill(&st.csv_path, st.use_auto_subtract, epoch_of(&st))?;
    let today = Local::now().date_naive();
    let now_str = Local::now().format("%H:%M:%S").to_string();
    Ok(tracker::today_info(
        &entries,
        st.use_auto_subtract,
        today,
        &now_str,
    ))
}

/// Set the MANUAL subtract for a date. Saved immediately.
#[tauri::command]
fn set_subtract(
    app: tauri::AppHandle,
    date: String,
    minutes: i64,
) -> Result<tracker::DayEntry, String> {
    let st = load_settings(&app);
    tracker::set_manual_subtract(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        &date,
        minutes,
    )
}

#[tauri::command]
fn set_use_auto(app: tauri::AppHandle, use_auto: bool) -> Result<Vec<tracker::DayEntry>, String> {
    let mut st = load_settings(&app);
    st.use_auto_subtract = use_auto;
    save_settings(&app, &st);
    tracker::recompute_all_tracked(&st.csv_path, use_auto)
}

#[tauri::command]
fn refresh(app: tauri::AppHandle) -> Result<Vec<tracker::DayEntry>, String> {
    let st = load_settings(&app);
    tracker::backfill(&st.csv_path, st.use_auto_subtract, epoch_of(&st))
}

#[tauri::command]
fn get_csv_path(app: tauri::AppHandle) -> String {
    load_settings(&app).csv_path
}

#[tauri::command]
fn import_backfill_csv(
    app: tauri::AppHandle,
    file_path: String,
) -> Result<tracker::ImportResult, String> {
    let st = load_settings(&app);
    tracker::import_file(&st.csv_path, &file_path)
}

/// Where the GUI can find a helper binary to stage: bundled resources first,
/// then anything next to the running executable (dev), then cargo targets.
fn helper_candidates(app: &tauri::AppHandle) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = app
        .path()
        .resolve("bin/wht-track", BaseDirectory::Resource)
    {
        v.push(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("wht-track"));
        }
    }
    let manifest = env!("CARGO_MANIFEST_DIR");
    v.push(PathBuf::from(format!(
        "{}/target/release/wht-track",
        manifest
    )));
    v.push(PathBuf::from(format!(
        "{}/target/debug/wht-track",
        manifest
    )));
    v
}

#[tauri::command]
fn background_status(app: tauri::AppHandle) -> tracker::BackgroundStatus {
    let st = load_settings(&app);
    let src = tracker::find_helper(&helper_candidates(&app));
    tracker::background_status(src, &st.csv_path, st.background_epoch.clone())
}

#[tauri::command]
fn install_background_job(app: tauri::AppHandle) -> Result<tracker::BackgroundStatus, String> {
    let st = load_settings(&app);
    let src = tracker::find_helper(&helper_candidates(&app)).ok_or_else(|| {
        "helper binary not found in the app bundle. Developers: run `cargo build --bin wht-track` (dev) or rebuild the app.".to_string()
    })?;
    tracker::install_background_job(&src)?;
    // Kick off one run right away so status/last-run is meaningful immediately.
    let uid = tracker::current_uid()?;
    let _ = std::process::Command::new("launchctl")
        .args(["kickstart", "-k", &format!("gui/{}/{}", uid, tracker::launchd_label())])
        .output();
    std::thread::sleep(std::time::Duration::from_secs(3));
    Ok(tracker::background_status(
        Some(src),
        &st.csv_path,
        load_settings(&app).background_epoch.clone(),
    ))
}

#[tauri::command]
fn uninstall_background_job(app: tauri::AppHandle) -> Result<tracker::BackgroundStatus, String> {
    let st = load_settings(&app);
    tracker::uninstall_background_job()?;
    Ok(tracker::background_status(
        None,
        &st.csv_path,
        st.background_epoch.clone(),
    ))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            get_entries,
            get_today,
            get_settings,
            set_subtract,
            set_use_auto,
            refresh,
            get_csv_path,
            import_backfill_csv,
            background_status,
            install_background_job,
            uninstall_background_job
        ])
        .run(tauri::generate_context!())
        .expect("error while running work-hours-tracker");
}

fn main() {
    run();
}
