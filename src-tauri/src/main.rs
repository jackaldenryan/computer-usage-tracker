mod tracker;

use std::path::PathBuf;
use tauri::path::BaseDirectory;
use tauri::{LogicalSize, Manager};

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
    tracker::backfill(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        st.day_start_hour,
        st.min_session_minutes,
    )
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> tracker::Settings {
    load_settings(&app)
}

#[tauri::command]
fn get_today(app: tauri::AppHandle) -> Result<tracker::TodayInfo, String> {
    let st = load_settings(&app);
    let entries = tracker::backfill(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        st.day_start_hour,
        st.min_session_minutes,
    )?;
    let today = tracker::logical_today(st.day_start_hour);
    let now_str = chrono::Local::now().format("%H:%M:%S").to_string();
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
        st.day_start_hour,
        st.min_session_minutes,
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
fn set_use_24h(app: tauri::AppHandle, use_24h: bool) -> tracker::Settings {
    let mut st = load_settings(&app);
    st.use_24h = use_24h;
    save_settings(&app, &st);
    st
}

#[tauri::command]
fn set_day_start_hour(
    app: tauri::AppHandle,
    hour: u32,
) -> Result<Vec<tracker::DayEntry>, String> {
    let mut st = load_settings(&app);
    st.day_start_hour = tracker::clamp_day_start_hour(hour);
    save_settings(&app, &st);
    tracker::backfill(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        st.day_start_hour,
        st.min_session_minutes,
    )
}

#[tauri::command]
fn set_min_session_minutes(
    app: tauri::AppHandle,
    minutes: u32,
) -> Result<Vec<tracker::DayEntry>, String> {
    let mut st = load_settings(&app);
    st.min_session_minutes = tracker::clamp_min_session_minutes(minutes);
    save_settings(&app, &st);
    tracker::backfill(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        st.day_start_hour,
        st.min_session_minutes,
    )
}

#[tauri::command]
fn refresh(app: tauri::AppHandle) -> Result<Vec<tracker::DayEntry>, String> {
    let st = load_settings(&app);
    tracker::backfill(
        &st.csv_path,
        st.use_auto_subtract,
        epoch_of(&st),
        st.day_start_hour,
        st.min_session_minutes,
    )
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

fn fit_main_window(app: &tauri::App) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    let Ok(Some(m)) = win.current_monitor() else {
        return;
    };
    let scale = m.scale_factor();
    let work = m.work_area();
    let avail_w = (work.size.width as f64 / scale).max(480.0);
    let avail_h = (work.size.height as f64 / scale).max(360.0);
    let w = (avail_w * 0.88).min(1200.0).min(avail_w - 24.0).max(560.0_f64.min(avail_w - 16.0));
    let h = (avail_h * 0.88).min(860.0).min(avail_h - 24.0).max(420.0_f64.min(avail_h - 16.0));
    let _ = win.set_min_size(Some(LogicalSize::new(
        560.0_f64.min(w),
        420.0_f64.min(h),
    )));
    let _ = win.set_size(LogicalSize::new(w, h));
    let _ = win.center();
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            fit_main_window(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_entries,
            get_today,
            get_settings,
            set_subtract,
            set_use_auto,
            set_use_24h,
            set_day_start_hour,
            set_min_session_minutes,
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
