//! wht-track: headless hourly tracker for Computer Usage Tracker.
//!
//! Reads the shared settings file (same one the GUI uses), runs the same
//! backfill over `pmset -g log`, writes the CSV, and stamps
//! `background_epoch` on first successful run. Safe to run any number of
//! times — every run reconstructs all missing days idempotently.
//!
//! Usage: wht-track [--settings PATH] [--csv PATH]

#[path = "../tracker.rs"]
mod tracker;

use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage: wht-track [--settings PATH] [--csv PATH]");
    std::process::exit(2);
}

fn main() {
    let mut settings_path: Option<String> = None;
    let mut csv_override: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--settings" => settings_path = args.next().or_else(|| usage()),
            "--csv" => csv_override = args.next().or_else(|| usage()),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }

    let sp: PathBuf = match settings_path {
        Some(p) => PathBuf::from(p),
        None => tracker::default_settings_path(),
    };
    let mut st = tracker::load_settings_from(&sp);
    let csv = csv_override.unwrap_or_else(|| st.csv_path.clone());
    let today = tracker::logical_today(st.day_start_hour);

    match tracker::backfill(
        &csv,
        st.use_auto_subtract,
        st.background_epoch.as_deref(),
        st.day_start_hour,
    ) {
        Ok(rows) => {
            // Stamp the epoch on first success: from here on, gaps the log
            // can't fill are recorded as 0h (machine would have been caught
            // by an hourly run had it been awake).
            if st.background_epoch.is_none() {
                st.background_epoch = Some(today.to_string());
            }
            if let Err(e) = tracker::save_settings_to(&sp, &st) {
                eprintln!("wht-track: warning: could not save settings: {}", e);
            }
            println!(
                "wht-track ok date={} rows={} epoch={} csv={}",
                today,
                rows.len(),
                st.background_epoch.as_deref().unwrap_or("-"),
                csv
            );
        }
        Err(e) => {
            eprintln!("wht-track error: {}", e);
            if e.contains("Operation not permitted") {
                eprintln!("wht-track hint: macOS denied file access. Either grant Full Disk Access to this binary (System Settings > Privacy & Security > Full Disk Access), or move the CSV somewhere unprotected (e.g. ~/Library/Application Support/com.workhours.tracker/).");
            }
            std::process::exit(1);
        }
    }
}
