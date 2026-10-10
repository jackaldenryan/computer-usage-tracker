//! Shared tracking logic used by both the Tauri GUI (`main.rs`) and the
//! headless hourly helper (`bin/wht-track.rs`).
//!
//! Main CSV columns:
//! date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked
//! - subtract_* empty = N/A (unknown), otherwise integer minutes.
//! - hours_worked = final trusted hours. For auto-tracked days it is
//!   (stop-start) minus the ACTIVE subtract mode; for uploaded rows it is
//!   whatever the user provided (start/stop may be empty).
//! - A date with NO row at all = N/A (no data, excluded from averages).
//! - A row with empty start/stop and hours 0 = confirmed no usage (counts as 0).
//!
//! Zero-fill rule for unfillable gaps: when the hourly background job exists,
//! any missing date on/after `background_epoch` that the pmset log can no
//! longer cover is recorded as 0h (same shape as confirmed no-use days). The
//! reasoning: had the machine been awake ~1h that day, the hourly run would
//! have captured it while the log still covered it — so no row means no
//! sustained usage (machine off/asleep all day is also 0h). Missing dates
//! BEFORE the epoch stay missing (= N/A): back then nobody was looking.

use chrono::{Local, NaiveDate, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayEntry {
    pub date: String,
    pub start: String,
    pub stop: String,
    pub subtract_manual_minutes: Option<i64>,
    pub subtract_auto_minutes: Option<i64>,
    pub hours_worked: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TodayInfo {
    pub date: String,
    pub start: String,
    pub stop_or_now: String,
    pub subtract_manual_minutes: Option<i64>,
    pub subtract_auto_minutes: Option<i64>,
    pub hours_so_far: f64,
    pub is_running: bool,
    pub use_auto: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_csv_path")]
    pub csv_path: String,
    #[serde(default)]
    pub use_auto_subtract: bool,
    #[serde(default)]
    pub use_24h: bool,
    #[serde(default = "default_day_start_hour")]
    pub day_start_hour: u32,
    #[serde(default = "default_min_session_minutes")]
    pub min_session_minutes: u32,
    /// First date the hourly background job successfully ran (YYYY-MM-DD).
    /// Set by wht-track, never by the GUI. Gaps on/after this date that the
    /// log can't fill are recorded as 0h; older gaps stay N/A.
    #[serde(default)]
    pub background_epoch: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

pub fn app_support_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(format!(
        "{}/Library/Application Support/com.workhours.tracker",
        home
    ))
}

pub fn default_day_start_hour() -> u32 {
    4
}

pub fn clamp_day_start_hour(h: u32) -> u32 {
    h.min(23)
}

pub fn default_min_session_minutes() -> u32 {
    10
}

pub fn clamp_min_session_minutes(m: u32) -> u32 {
    m.min(240)
}

pub fn default_csv_path() -> String {
    // Deliberately NOT ~/Documents: macOS gates Documents/Desktop/Downloads
    // behind per-app privacy consent that a headless background process can
    // never obtain (no windows -> no prompt -> silent deny). Application
    // Support needs no consent, so the hourly helper works with zero user
    // interaction. Users may still relocate into Documents from Settings, but
    // then the background job needs a manual Full Disk Access grant.
    app_support_dir()
        .join("work-hours.csv")
        .to_string_lossy()
        .to_string()
}

/// Standard settings location shared by the GUI and the helper:
/// ~/Library/Application Support/com.workhours.tracker/settings.json
/// (matches Tauri's app_data_dir for this bundle on macOS).
pub fn default_settings_path() -> PathBuf {
    app_support_dir().join("settings.json")
}

pub fn load_settings_from(p: &Path) -> Settings {
    let mut st = if let Ok(s) = std::fs::read_to_string(p) {
        if let Ok(parsed) = serde_json::from_str::<Settings>(&s) {
            if !parsed.csv_path.is_empty() {
                parsed
            } else {
                Settings {
                    csv_path: default_csv_path(),
                    use_auto_subtract: false,
                    use_24h: false,
                    day_start_hour: default_day_start_hour(),
                    min_session_minutes: default_min_session_minutes(),
                    background_epoch: None,
                }
            }
        } else {
            Settings {
                csv_path: default_csv_path(),
                use_auto_subtract: false,
                use_24h: false,
                day_start_hour: default_day_start_hour(),
                min_session_minutes: default_min_session_minutes(),
                background_epoch: None,
            }
        }
    } else {
        Settings {
            csv_path: default_csv_path(),
            use_auto_subtract: false,
            use_24h: false,
            day_start_hour: default_day_start_hour(),
            min_session_minutes: default_min_session_minutes(),
            background_epoch: None,
        }
    };
    // Single canonical location: the CSV lives at the default path, nowhere
    // else. If settings point elsewhere, migrate once:
    // - default exists -> it wins (stray file left untouched, never deleted);
    // - else if the configured file exists -> move it into place;
    // - else -> just use the default.
    // A failed move falls back to the configured path (better than stranding
    // data); the next launch retries.
    let def = default_csv_path();
    if st.csv_path != def {
        if Path::new(&def).exists() {
            st.csv_path = def;
            let _ = save_settings_to(p, &st);
        } else if Path::new(&st.csv_path).exists() {
            if let Some(parent) = Path::new(&def).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let moved = std::fs::rename(&st.csv_path, &def).is_ok() || {
                std::fs::copy(&st.csv_path, &def)
                    .and_then(|_| std::fs::remove_file(&st.csv_path))
                    .is_ok()
            };
            if moved {
                st.csv_path = def;
                let _ = save_settings_to(p, &st);
            }
        } else {
            st.csv_path = def;
            let _ = save_settings_to(p, &st);
        }
    }
    st
}

pub fn save_settings_to(p: &Path, st: &Settings) -> Result<(), String> {
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let s = serde_json::to_string_pretty(st).map_err(|e| e.to_string())?;
    std::fs::write(p, s).map_err(|e| e.to_string())
}

/// Run `pmset -g log` once and return stdout.
pub fn pmset_log() -> String {
    match Command::new("pmset").args(["-g", "log"]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => String::new(),
    }
}

pub fn assign_logical_date(cal: NaiveDate, t: NaiveTime, day_start_hour: u32) -> NaiveDate {
    if t.hour() < clamp_day_start_hour(day_start_hour) {
        cal.pred_opt().unwrap_or(cal)
    } else {
        cal
    }
}

pub fn logical_today(day_start_hour: u32) -> NaiveDate {
    let n = Local::now().naive_local();
    assign_logical_date(n.date(), n.time(), day_start_hour)
}

fn parse_log_stamp(line: &str) -> Option<(NaiveDate, NaiveTime, String)> {
    if line.len() < 19 {
        return None;
    }
    let cal = NaiveDate::parse_from_str(line.get(0..10)?, "%Y-%m-%d").ok()?;
    let tstr = line.get(11..19)?.to_string();
    let t = parse_hms(&tstr)?;
    Some((cal, t, tstr))
}

fn line_on_logical_day(line: &str, target: &str, day_start_hour: u32) -> Option<String> {
    let (cal, t, tstr) = parse_log_stamp(line)?;
    if assign_logical_date(cal, t, day_start_hour).to_string() == target {
        Some(tstr)
    } else {
        None
    }
}

fn secs_from_day_start(t: NaiveTime, day_start_hour: u32) -> i64 {
    let s = t.num_seconds_from_midnight() as i64;
    let start = clamp_day_start_hour(day_start_hour) as i64 * 3600;
    if s >= start {
        s - start
    } else {
        s + 86400 - start
    }
}

/// First Display-on / last Display-off of sessions lasting at least
/// `min_session_minutes`. Brief overnight display blips are ignored.
/// `now` is used so an in-progress session today can qualify.
pub fn parse_day(
    log: &str,
    target: &str,
    day_start_hour: u32,
    min_session_minutes: u32,
    now: Option<NaiveTime>,
) -> (Option<String>, Option<String>) {
    let mut events: Vec<(i64, String, bool)> = Vec::new();
    let mut first_wake: Option<(i64, String)> = None;
    let mut saw_display = false;
    for line in log.lines() {
        let Some(tstr) = line_on_logical_day(line, target, day_start_hour) else {
            continue;
        };
        let Some(t) = parse_hms(&tstr) else { continue };
        let off = secs_from_day_start(t, day_start_hour);
        if line.contains("Display is turned on") {
            saw_display = true;
            events.push((off, tstr, true));
        } else if line.contains("Display is turned off") {
            events.push((off, tstr, false));
        } else if line.contains("Wake from") && !line.contains("DarkWake") {
            if first_wake.as_ref().map(|(o, _)| off < *o).unwrap_or(true) {
                first_wake = Some((off, tstr));
            }
        }
    }
    events.sort_by_key(|(o, _, _)| *o);
    let min_secs = clamp_min_session_minutes(min_session_minutes) as i64 * 60;
    let now_off = now.map(|t| secs_from_day_start(t, day_start_hour));
    let mut sessions: Vec<(i64, String, Option<(i64, String)>)> = Vec::new();
    let mut open: Option<(i64, String)> = None;
    for (off, tstr, is_on) in events {
        if is_on {
            if open.is_none() {
                open = Some((off, tstr));
            }
        } else if let Some((on_o, on_t)) = open.take() {
            sessions.push((on_o, on_t, Some((off, tstr))));
        }
    }
    if let Some((on_o, on_t)) = open {
        sessions.push((on_o, on_t, None));
    }
    let mut first_on = None;
    let mut last_off = None;
    for (on_o, on_t, off) in sessions {
        let dur = match &off {
            Some((off_o, _)) => *off_o - on_o,
            None => now_off.filter(|n| *n > on_o).map(|n| n - on_o).unwrap_or(0),
        };
        if dur < min_secs {
            continue;
        }
        if first_on.is_none() {
            first_on = Some(on_t);
        }
        last_off = off.map(|(_, s)| s);
    }
    if first_on.is_none() && !saw_display {
        return (first_wake.map(|(_, s)| s), None);
    }
    (first_on, last_off)
}

/// Automatically-calculated minutes to subtract: total time the display was
/// off in gaps BETWEEN the first on and the last event of the day.
/// Returns None when the day has no on/wake event at all (nothing to measure).
pub fn compute_auto_gap_minutes(log: &str, target: &str, day_start_hour: u32) -> Option<i64> {
    let mut events: Vec<(i64, bool)> = Vec::new();
    let mut saw_any_on = false;
    for line in log.lines() {
        let Some(tstr) = line_on_logical_day(line, target, day_start_hour) else {
            continue;
        };
        let Some(t) = parse_hms(&tstr) else { continue };
        let off = secs_from_day_start(t, day_start_hour);
        if line.contains("Display is turned on") {
            saw_any_on = true;
            events.push((off, true));
        } else if line.contains("Display is turned off") {
            events.push((off, false));
        } else if line.contains("Wake from") && !line.contains("DarkWake") {
            saw_any_on = true;
        }
    }
    if !saw_any_on {
        return None;
    }
    events.sort_by_key(|(o, _)| *o);
    let mut gap_secs: i64 = 0;
    let mut off_since: Option<i64> = None;
    for (t, is_on) in events {
        if is_on {
            if let Some(off) = off_since.take() {
                let d = t - off;
                if d > 0 {
                    gap_secs += d;
                }
            }
        } else if off_since.is_none() {
            off_since = Some(t);
        }
    }
    Some((gap_secs as f64 / 60.0).round() as i64)
}

/// All dates in pmset log that have any Display-on event.
pub fn earliest_logged_date(log: &str, day_start_hour: u32) -> Option<String> {
    let mut min: Option<String> = None;
    for line in log.lines() {
        if !line.contains("Display is turned on") {
            continue;
        }
        let Some((cal, t, _)) = parse_log_stamp(line) else {
            continue;
        };
        let d = assign_logical_date(cal, t, day_start_hour).to_string();
        if min.as_ref().map(|m| d.as_str() < m.as_str()).unwrap_or(true) {
            min = Some(d);
        }
    }
    min
}

pub fn parse_hms(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S").ok()
}

pub fn gross_hours(start: &str, stop: &str) -> f64 {
    if start.is_empty() || stop.is_empty() {
        return 0.0;
    }
    let (st, en) = match (parse_hms(start), parse_hms(stop)) {
        (Some(a), Some(b)) => (a, b),
        _ => return 0.0,
    };
    let mut secs = (en - st).num_seconds();
    if secs < 0 {
        secs += 24 * 3600;
    }
    if secs <= 0 {
        return 0.0;
    }
    let secs = secs as f64;
    (secs / 3600.0 * 100.0).round() / 100.0
}

pub fn net_hours(start: &str, stop: &str, sub: Option<i64>) -> f64 {
    let g = gross_hours(start, stop);
    if g <= 0.0 {
        return 0.0;
    }
    match sub {
        None => g, // N/A subtract -> trust gross (uploads with no subtract col)
        Some(m) => {
            let h = g - (m as f64) / 60.0;
            ((h.max(0.0)) * 100.0).round() / 100.0
        }
    }
}

pub fn active_sub(e: &DayEntry, use_auto: bool) -> Option<i64> {
    if use_auto {
        e.subtract_auto_minutes
    } else {
        e.subtract_manual_minutes
    }
}

/// Hours to DISPLAY for stats. None = N/A (excluded from averages).
/// - Tracked rows (has start or stop): gross minus active subtract.
///   Auto is always Some when start exists, so this is rarely N/A.
/// - No-event rows (empty start/stop, hours 0): Some(0.0) confirmed no usage.
/// - Uploaded rows (empty start/stop, hours > 0): trusted hours_worked.
pub fn display_hours(e: &DayEntry, use_auto: bool) -> Option<f64> {
    if !e.start.is_empty() || !e.stop.is_empty() {
        if e.start.is_empty() || e.stop.is_empty() {
            // incomplete (e.g. running today with no close yet, or crash):
            // if running today the caller overrides with live calc; otherwise N/A
            if e.stop.is_empty() && !e.start.is_empty() {
                return None;
            }
            return None;
        }
        Some(net_hours(&e.start, &e.stop, active_sub(e, use_auto)))
    } else if e.hours_worked > 0.0 {
        Some(e.hours_worked) // uploaded final hours, trusted in both modes
    } else {
        Some(0.0) // confirmed no usage
    }
}

pub fn parse_opt_i64(s: &str) -> Option<i64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<i64>().ok()
}

/// Flexible CSV load: supports new headers plus legacy `subtract_minutes`
/// (= manual). Returns map keyed by date.
pub fn load_entries_from_csv(path: &str) -> BTreeMap<String, DayEntry> {
    let mut map = BTreeMap::new();
    let Ok(mut rdr) = csv::ReaderBuilder::new()
        .flexible(true)
        .from_path(path)
    else {
        return map;
    };
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(_) => return map,
    };
    let norm: Vec<String> = headers
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let col = |names: &[&str]| -> Option<usize> {
        for n in names {
            if let Some(i) = norm.iter().position(|h| h == n) {
                return Some(i);
            }
        }
        None
    };
    let c_date = col(&["date", "day"]);
    let c_start = col(&["start", "start_time"]);
    let c_stop = col(&["stop", "stop_time", "end", "end_time"]);
    let c_man = col(&[
        "subtract_manual_minutes",
        "subtract_manual",
        "manual_minutes",
        "manual",
        "subtract_minutes", // legacy = manual
        "subtract",
    ]);
    let c_auto = col(&[
        "subtract_auto_minutes",
        "subtract_auto",
        "auto_minutes",
        "auto",
    ]);
    let c_hours = col(&[
        "hours_worked",
        "hours",
        "total_hours",
        "time_worked",
        "total",
        "hrs",
    ]);
    let c_date = match c_date {
        Some(i) => i,
        None => return map, // date column required
    };
    for rec in rdr.records() {
        let Ok(r) = rec else { continue };
        let get = |i: Option<usize>| -> String {
            i.and_then(|idx| r.get(idx)).unwrap_or("").trim().to_string()
        };
        let date = get(Some(c_date));
        if NaiveDate::parse_from_str(&date, "%Y-%m-%d").is_err() {
            continue;
        }
        let start = get(c_start);
        let stop = get(c_stop);
        let man = match c_man {
            Some(i) => parse_opt_i64(r.get(i).unwrap_or("")),
            None => None,
        };
        let auto = match c_auto {
            Some(i) => parse_opt_i64(r.get(i).unwrap_or("")),
            None => None,
        };
        let hours_raw = c_hours
            .and_then(|i| r.get(i))
            .unwrap_or("")
            .trim()
            .to_string();
        let mut hours = hours_raw
            .parse::<f64>()
            .ok()
            .filter(|h| *h >= 0.0)
            .unwrap_or(0.0);
        if hours_raw.is_empty() && r.len() < headers.len() {
            if let Some(last) = r.get(r.len().saturating_sub(1)) {
                if let Ok(h) = last.trim().parse::<f64>() {
                    if h >= 0.0 {
                        hours = (h * 100.0).round() / 100.0;
                    }
                }
            }
        }
        let hours: f64 = hours.max(0.0);
        map.insert(
            date.clone(),
            DayEntry {
                date,
                start,
                stop,
                subtract_manual_minutes: man,
                subtract_auto_minutes: auto,
                hours_worked: (hours * 100.0).round() / 100.0,
            },
        );
    }
    map
}

pub fn write_entries_to_csv(path: &str, map: &BTreeMap<String, DayEntry>) -> Result<(), String> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let mut wtr = csv::Writer::from_path(path).map_err(|e| e.to_string())?;
    for (_, e) in map.iter() {
        wtr.serialize(e).map_err(|e| e.to_string())?;
    }
    wtr.flush().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn is_uploaded(e: &DayEntry) -> bool {
    e.start.is_empty() && e.stop.is_empty() && e.hours_worked > 0.0
}

fn zero_row(date: String) -> DayEntry {
    DayEntry {
        date,
        start: String::new(),
        stop: String::new(),
        subtract_manual_minutes: Some(0),
        subtract_auto_minutes: None,
        hours_worked: 0.0,
    }
}

/// Core backfill over an explicit log snapshot and "today", so tests can
/// drive it deterministically. `epoch`: first date the hourly background job
/// ran (None = no background job: pre-log gaps stay missing = N/A).
pub fn backfill_impl(
    csv_path: &str,
    use_auto: bool,
    epoch: Option<&str>,
    log: &str,
    today: NaiveDate,
    day_start_hour: u32,
    min_session_minutes: u32,
) -> Result<Vec<DayEntry>, String> {
    // Never overwrite an existing file we cannot read (e.g. TCC denying a
    // background process access to ~/Documents): that would silently destroy
    // data by rewriting a log-only subset.
    if Path::new(csv_path).exists() {
        std::fs::read_to_string(csv_path)
            .map_err(|e| format!("cannot read existing CSV '{}': {}. If running headless, the CSV may live somewhere macOS privacy controls block (e.g. ~/Documents needs a Full Disk Access grant for the helper, or relocate the CSV).", csv_path, e))?;
    }
    let mut map = load_entries_from_csv(csv_path);
    let today_s = today.to_string();

    // Pass 1: fill missing days within pmset log coverage [log_start .. today].
    let log_start = earliest_logged_date(log, day_start_hour).unwrap_or_else(|| today_s.clone());
    let Ok(mut d) = NaiveDate::parse_from_str(&log_start, "%Y-%m-%d") else {
        return Ok(map.values().cloned().collect());
    };
    let today_d = today;

    while d <= today_d {
        let ds = d.to_string();
        if map.get(&ds).map(is_uploaded).unwrap_or(false) {
            d = d.succ_opt().unwrap_or(today_d);
            if d > today_d {
                break;
            }
            continue;
        }
        let prev = map.get(&ds).cloned();
        let now = if ds == today_s {
            Some(Local::now().naive_local().time())
        } else {
            None
        };
        let (start, stop) = parse_day(log, &ds, day_start_hour, min_session_minutes, now);
        let s = start.unwrap_or_default();
        let e = stop.unwrap_or_default();
        let auto = compute_auto_gap_minutes(log, &ds, day_start_hour);
        let auto = if s.is_empty() { None } else { auto.or(Some(0)) };
        let man = prev
            .as_ref()
            .and_then(|p| p.subtract_manual_minutes)
            .or(Some(0));
        let hours = if s.is_empty() && e.is_empty() {
            0.0
        } else if e.is_empty() {
            0.0
        } else {
            let sub = if use_auto { auto } else { man };
            net_hours(&s, &e, sub)
        };
        map.insert(
            ds.clone(),
            DayEntry {
                date: ds,
                start: s,
                stop: e,
                subtract_manual_minutes: man,
                subtract_auto_minutes: auto,
                hours_worked: hours,
            },
        );
        d = d.succ_opt().unwrap_or(today_d);
        if d > today_d {
            break;
        }
    }

    // Pass 2 (needs background_epoch): missing dates on/after the epoch that
    // the log can no longer cover are recorded as 0h — had the machine been
    // awake ~1h any of those days, the hourly run would have captured it while
    // the log still covered it. Missing dates BEFORE the epoch stay N/A.
    if let Some(ep_s) = epoch {
        if let Ok(ep) = NaiveDate::parse_from_str(ep_s, "%Y-%m-%d") {
            let log_start_d =
                NaiveDate::parse_from_str(&log_start, "%Y-%m-%d").unwrap_or(today_d);
            let mut g = ep;
            while g < log_start_d && g <= today_d {
                let gs = g.to_string();
                if !map.contains_key(&gs) {
                    map.insert(gs.clone(), zero_row(gs));
                }
                g = g.succ_opt().unwrap_or(log_start_d);
                if g >= log_start_d {
                    break;
                }
            }
        }
    }

    write_entries_to_csv(csv_path, &map)?;
    Ok(map.values().cloned().collect())
}

/// Ensure CSV has one row per day for all log-covered dates through today
/// (plus epoch-scoped zero-fill for older gaps). Preserves existing data.
pub fn backfill(
    csv_path: &str,
    use_auto: bool,
    epoch: Option<&str>,
    day_start_hour: u32,
    min_session_minutes: u32,
) -> Result<Vec<DayEntry>, String> {
    let log = pmset_log();
    let today = logical_today(day_start_hour);
    backfill_impl(
        csv_path,
        use_auto,
        epoch,
        &log,
        today,
        day_start_hour,
        min_session_minutes,
    )
}

pub fn recompute_all_tracked(path: &str, use_auto: bool) -> Result<Vec<DayEntry>, String> {
    let mut map = load_entries_from_csv(path);
    for (_, e) in map.iter_mut() {
        if is_uploaded(e) {
            continue;
        }
        if !e.start.is_empty() && !e.stop.is_empty() {
            e.hours_worked = net_hours(&e.start, &e.stop, active_sub(e, use_auto));
        }
    }
    write_entries_to_csv(path, &map)?;
    Ok(map.values().cloned().collect())
}

/// Live info for today (per the active subtract mode).
pub fn today_info(
    entries: &[DayEntry],
    use_auto: bool,
    today: NaiveDate,
    now_hms: &str,
) -> TodayInfo {
    let today_s = today.to_string();
    let row = entries.iter().find(|e| e.date == today_s);
    let (start, stop, man, auto) = match row {
        Some(r) => (
            r.start.clone(),
            r.stop.clone(),
            r.subtract_manual_minutes,
            r.subtract_auto_minutes,
        ),
        None => (String::new(), String::new(), Some(0), None),
    };
    let is_running = !start.is_empty() && stop.is_empty();
    let hours = if start.is_empty() {
        row.map(|r| {
            if is_uploaded(r) {
                r.hours_worked
            } else {
                0.0
            }
        })
        .unwrap_or(0.0)
    } else if is_running {
        let sub = if use_auto { auto } else { man };
        net_hours(&start, now_hms, sub)
    } else {
        row.map(|r| display_hours(r, use_auto).unwrap_or(0.0))
            .unwrap_or(0.0)
    };
    TodayInfo {
        date: today_s,
        start: start.clone(),
        stop_or_now: if is_running {
            now_hms.to_string()
        } else {
            stop
        },
        subtract_manual_minutes: man,
        subtract_auto_minutes: auto,
        hours_so_far: hours,
        is_running,
        use_auto,
    }
}

/// Set the MANUAL subtract for a date. Saved immediately. Stored hours are
/// recomputed per the active mode for tracked rows; uploaded rows keep their
/// trusted hours.
pub fn set_manual_subtract(
    csv_path: &str,
    use_auto: bool,
    epoch: Option<&str>,
    day_start_hour: u32,
    min_session_minutes: u32,
    date: &str,
    minutes: i64,
) -> Result<DayEntry, String> {
    let _ = backfill(csv_path, use_auto, epoch, day_start_hour, min_session_minutes);
    let mut map = load_entries_from_csv(csv_path);
    let mins = minutes.max(0).min(1440);
    if let Some(e) = map.get_mut(date) {
        e.subtract_manual_minutes = Some(mins);
        if !is_uploaded(e) && !e.start.is_empty() && !e.stop.is_empty() {
            e.hours_worked = net_hours(&e.start, &e.stop, active_sub(e, use_auto));
        }
        let out = e.clone();
        write_entries_to_csv(csv_path, &map)?;
        return Ok(out);
    }
    let e = DayEntry {
        date: date.to_string(),
        start: String::new(),
        stop: String::new(),
        subtract_manual_minutes: Some(mins),
        subtract_auto_minutes: None,
        hours_worked: 0.0,
    };
    map.insert(date.to_string(), e.clone());
    write_entries_to_csv(csv_path, &map)?;
    Ok(e)
}

#[derive(Debug, Clone)]
pub struct ImportRow {
    pub date: String,
    pub hours: f64,
    pub start: String,
    pub stop: String,
    pub man_provided: bool,
    pub auto_provided: bool,
    pub man: Option<i64>,
    pub auto_: Option<i64>,
}

pub fn find_col(norm: &[String], names: &[&str]) -> Option<usize> {
    for n in names {
        if let Some(i) = norm.iter().position(|h| h == n) {
            return Some(i);
        }
    }
    None
}

/// Parse backfill CSV text into rows. Required: date + total hours (final).
/// Optional: start, stop, subtract manual/auto. Missing subtracts = N/A.
/// Returns (rows, skipped, errors).
pub fn parse_import_csv(content: &str) -> Result<(Vec<ImportRow>, usize, Vec<String>), String> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(content.as_bytes());
    let headers = rdr.headers().map_err(|e| e.to_string())?.clone();
    let norm: Vec<String> = headers
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect();
    let c_date = find_col(&norm, &["date", "day"]);
    let c_hours = find_col(
        &norm,
        &[
            "hours_worked",
            "hours",
            "total_hours",
            "time_worked",
            "total",
            "hrs",
        ],
    );
    let (c_date, c_hours) = match (c_date, c_hours) {
        (Some(a), Some(b)) => (a, b),
        _ => {
            return Err(
                "Backfill CSV must have a date column and an hours column (hours_worked)."
                    .to_string(),
            )
        }
    };
    let c_start = find_col(&norm, &["start", "start_time"]);
    let c_stop = find_col(&norm, &["stop", "stop_time", "end", "end_time"]);
    let c_man = find_col(
        &norm,
        &[
            "subtract_manual_minutes",
            "subtract_manual",
            "manual_minutes",
            "manual",
            "subtract_minutes",
            "subtract",
        ],
    );
    let c_auto = find_col(
        &norm,
        &[
            "subtract_auto_minutes",
            "subtract_auto",
            "auto_minutes",
            "auto",
        ],
    );

    let mut rows = Vec::new();
    let mut skipped = 0;
    let mut errors = Vec::new();

    for (lineno, rec) in rdr.records().enumerate() {
        let r = match rec {
            Ok(r) => r,
            Err(e) => {
                skipped += 1;
                errors.push(format!("line {}: {}", lineno + 2, e));
                continue;
            }
        };
        let raw_date = r.get(c_date).unwrap_or("").trim().to_string();
        let date = if NaiveDate::parse_from_str(&raw_date, "%Y-%m-%d").is_ok() {
            raw_date
        } else if let Ok(d) = NaiveDate::parse_from_str(&raw_date, "%m/%d/%Y") {
            d.to_string()
        } else {
            skipped += 1;
            errors.push(format!("line {}: bad date '{}'", lineno + 2, raw_date));
            continue;
        };
        let hours: f64 = match r.get(c_hours).unwrap_or("").trim().parse::<f64>() {
            Ok(h) if h >= 0.0 => (h * 100.0).round() / 100.0,
            _ => {
                skipped += 1;
                errors.push(format!("line {}: bad hours", lineno + 2));
                continue;
            }
        };
        let start = c_start
            .and_then(|i| r.get(i))
            .unwrap_or("")
            .trim()
            .to_string();
        let stop = c_stop
            .and_then(|i| r.get(i))
            .unwrap_or("")
            .trim()
            .to_string();
        if !start.is_empty() && parse_hms(&start).is_none() {
            skipped += 1;
            errors.push(format!("line {}: bad start '{}'", lineno + 2, start));
            continue;
        }
        if !stop.is_empty() && parse_hms(&stop).is_none() {
            skipped += 1;
            errors.push(format!("line {}: bad stop '{}'", lineno + 2, stop));
            continue;
        }
        let man_provided = c_man.is_some();
        let auto_provided = c_auto.is_some();
        let man = c_man.and_then(|i| parse_opt_i64(r.get(i).unwrap_or("")));
        let auto = c_auto.and_then(|i| parse_opt_i64(r.get(i).unwrap_or("")));
        if man_provided
            && c_man
                .and_then(|i| r.get(i))
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
            && man.is_none()
        {
            skipped += 1;
            errors.push(format!("line {}: bad manual subtract", lineno + 2));
            continue;
        }
        if auto_provided
            && c_auto
                .and_then(|i| r.get(i))
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
            && auto.is_none()
        {
            skipped += 1;
            errors.push(format!("line {}: bad auto subtract", lineno + 2));
            continue;
        }
        rows.push(ImportRow {
            date,
            hours,
            start,
            stop,
            man_provided,
            auto_provided,
            man,
            auto_: auto,
        });
    }
    Ok((rows, skipped, errors))
}

/// Merge uploaded rows into the main map. Uploaded hours are trusted as final.
/// Existing dates: hours replaced; subtracts/start/stop only when provided.
pub fn apply_import(
    map: &mut BTreeMap<String, DayEntry>,
    rows: Vec<ImportRow>,
) -> (usize, usize) {
    let mut imported = 0;
    let mut updated = 0;
    for row in rows {
        match map.get_mut(&row.date) {
            Some(e) => {
                e.hours_worked = row.hours;
                if row.man_provided {
                    e.subtract_manual_minutes = row.man;
                }
                if row.auto_provided {
                    e.subtract_auto_minutes = row.auto_;
                }
                if !row.start.is_empty() {
                    e.start = row.start;
                }
                if !row.stop.is_empty() {
                    e.stop = row.stop;
                }
                updated += 1;
            }
            None => {
                map.insert(
                    row.date.clone(),
                    DayEntry {
                        date: row.date,
                        start: row.start,
                        stop: row.stop,
                        subtract_manual_minutes: row.man,
                        subtract_auto_minutes: row.auto_,
                        hours_worked: row.hours,
                    },
                );
                imported += 1;
            }
        }
    }
    (imported, updated)
}

/// Import a backfill CSV file into the main CSV. NOTE: no backfill pass here
/// on purpose — uploads may cover dates far older than pmset coverage, and we
/// must not fabricate zero rows for the gap (those stay N/A unless the epoch
/// rule applies on the next regular backfill).
pub fn import_file(
    csv_path: &str,
    file_path: &str,
) -> Result<ImportResult, String> {
    if Path::new(csv_path).exists() {
        std::fs::read_to_string(csv_path)
            .map_err(|e| format!("cannot read existing CSV '{}': {}", csv_path, e))?;
    }
    let mut map = load_entries_from_csv(csv_path);
    let content = std::fs::read_to_string(file_path).map_err(|e| e.to_string())?;
    let (rows, skipped, errors) = parse_import_csv(&content)?;
    let (imported, updated) = apply_import(&mut map, rows);
    write_entries_to_csv(csv_path, &map)?;
    Ok(ImportResult {
        imported,
        updated,
        skipped,
        errors,
    })
}

// ---------------------------------------------------------------------------
// Background (hourly, headless) job management.
//
// Everything here is designed to be driven from inside the GUI: no Terminal,
// no System Settings. The only hard platform limit is that a CSV inside
// ~/Documents (or Desktop/Downloads) is unreachable to the helper without a
// manual Full Disk Access grant — hence the App Support default above and
// `csv_needs_consent()` below, which the UI uses to warn.
// ---------------------------------------------------------------------------

pub fn launchd_label() -> &'static str {
    "com.workhours.tracker.track"
}

pub fn helper_dest() -> PathBuf {
    app_support_dir().join("bin").join("wht-track")
}

pub fn plist_dest() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(format!(
        "{}/Library/LaunchAgents/{}.plist",
        home,
        launchd_label()
    ))
}

pub fn bg_log_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(format!(
        "{}/Library/Logs/work-hours-tracker/wht-track.log",
        home
    ))
}

/// True when the CSV lives somewhere macOS gates behind per-app privacy
/// consent (unreachable to the headless helper without manual FDA grant).
pub fn csv_needs_consent(csv_path: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    let lower = csv_path.to_lowercase();
    if home.is_empty() {
        return false;
    }
    let h = home.to_lowercase();
    for gated in ["documents", "desktop", "downloads"] {
        if lower == format!("{}/{}", h, gated) || lower.starts_with(&format!("{}/{}/", h, gated))
        {
            return true;
        }
    }
    false
}

pub fn current_uid() -> Result<String, String> {
    let out = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("could not determine uid".to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn render_launchd_plist(bin: &Path, log: &Path) -> String {
    include_str!("../../background/com.workhours.tracker.track.plist.template")
        .replace("__BIN_PATH__", &bin.to_string_lossy())
        .replace("__LOG_PATH__", &log.to_string_lossy())
}

fn launchctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("launchctl")
        .args(args)
        .output()
        .map_err(|e| format!("launchctl failed to run: {}", e))?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(combined)
    } else {
        Err(format!("launchctl {} failed: {}", args.join(" "), combined.trim()))
    }
}

pub fn find_helper(candidates: &[PathBuf]) -> Option<PathBuf> {
    // Non-empty check: the repo carries a 0-byte placeholder so the Tauri
    // build script's resource validation passes before the real binary is
    // staged; never install that.
    candidates
        .iter()
        .find(|p| {
            p.is_file()
                && std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
        })
        .cloned()
}

/// Install (or refresh) the hourly job from a helper binary the GUI ships.
/// Overwrites the helper copy + plist, bootstraps the agent. Idempotent.
pub fn install_background_job(helper_src: &Path) -> Result<(), String> {
    let dest = helper_dest();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(helper_src, &dest)
        .map_err(|e| format!("could not stage helper binary: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    let log = bg_log_path();
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let plist = plist_dest();
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&plist, render_launchd_plist(&dest, &log)).map_err(|e| e.to_string())?;
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, launchd_label());
    let _ = launchctl(&["bootout", &target]); // ignore if not loaded
    launchctl(&["bootstrap", &format!("gui/{}", uid), &plist.to_string_lossy()])?;
    Ok(())
}

pub fn uninstall_background_job() -> Result<(), String> {
    let uid = current_uid()?;
    let target = format!("gui/{}/{}", uid, launchd_label());
    let _ = launchctl(&["bootout", &target]); // ignore if not loaded
    let plist = plist_dest();
    if plist.exists() {
        std::fs::remove_file(&plist).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundStatus {
    pub helper_available: bool,
    pub helper_path: String,
    pub installed: bool,
    pub loaded: bool,
    pub last_line: Option<String>,
    pub csv_protected: bool,
    pub epoch: Option<String>,
}

pub fn background_status(
    helper_src: Option<PathBuf>,
    csv_path: &str,
    epoch: Option<String>,
) -> BackgroundStatus {
    let uid = current_uid().unwrap_or_else(|_| "501".to_string());
    let target = format!("gui/{}/{}", uid, launchd_label());
    let loaded = launchctl(&["print", &target]).is_ok();
    let last_line = std::fs::read_to_string(bg_log_path()).ok().and_then(|s| {
        s.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| {
                let mut t = l.trim().to_string();
                if t.len() > 200 {
                    t.truncate(200);
                }
                t
            })
    });
    BackgroundStatus {
        helper_available: helper_src.is_some(),
        helper_path: helper_src
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        installed: plist_dest().exists(),
        loaded,
        last_line,
        csv_protected: csv_needs_consent(csv_path),
        epoch,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day_log(date: &str, on1: &str, off1: &str, on2: Option<&str>, off2: &str) -> String {
        let mut s = format!(
            "{} {} -0500 Notification  \tDisplay is turned on\n{} {} -0500 Notification  \tDisplay is turned off\n",
            date, on1, date, off1
        );
        if let Some(o2) = on2 {
            s.push_str(&format!(
                "{} {} -0500 Notification  \tDisplay is turned on\n",
                date, o2
            ));
        }
        s.push_str(&format!(
            "{} {} -0500 Notification  \tDisplay is turned off\n",
            date, off2
        ));
        s
    }

    #[test]
    fn import_minimal_hours_only() {
        let (rows, skipped, _) =
            parse_import_csv("date,hours_worked\n2026-09-20,7.5\n2026-09-21,0\n").unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].date, "2026-09-20");
        assert_eq!(rows[0].hours, 7.5);
        assert!(!rows[0].man_provided && !rows[0].auto_provided);
        assert_eq!(rows[0].man, None);
        let mut map = BTreeMap::new();
        let (i, u) = apply_import(&mut map, rows);
        assert_eq!((i, u), (2, 0));
        let e = &map["2026-09-20"];
        assert_eq!(e.hours_worked, 7.5);
        assert_eq!(e.start, "");
        assert_eq!(e.subtract_manual_minutes, None); // N/A, not zero
    }

    #[test]
    fn import_with_subtracts_and_merge() {
        let mut map = BTreeMap::new();
        map.insert(
            "2026-10-03".to_string(),
            DayEntry {
                date: "2026-10-03".to_string(),
                start: "01:45:24".to_string(),
                stop: "16:10:18".to_string(),
                subtract_manual_minutes: Some(0),
                subtract_auto_minutes: Some(801),
                hours_worked: 1.07,
            },
        );
        // upload provides hours + manual only: auto + start/stop preserved
        let (rows, _, _) =
            parse_import_csv("date,hours,subtract_manual\n2026-10-03,6.5,30\n").unwrap();
        let (i, u) = apply_import(&mut map, rows);
        assert_eq!((i, u), (0, 1));
        let e = &map["2026-10-03"];
        assert_eq!(e.hours_worked, 6.5);
        assert_eq!(e.subtract_manual_minutes, Some(30));
        assert_eq!(e.subtract_auto_minutes, Some(801));
        assert_eq!(e.start, "01:45:24");
    }

    #[test]
    fn short_rows_with_missing_trailing_fields_survive() {
        // e.g. Sheets trims trailing empties: 5 fields against 6 headers
        let (rows, skipped, _) =
            parse_import_csv("date,hours_worked\n2026-09-20,7.5\n").unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 1);
        let dir = std::env::temp_dir();
        let p = dir.join("wht_short_test.csv");
        std::fs::write(
            &p,
            "date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked\n2026-09-20,,,,7.5\n",
        )
        .unwrap();
        let map = load_entries_from_csv(p.to_str().unwrap());
        assert_eq!(map["2026-09-20"].hours_worked, 7.5);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn import_rejects_bad_rows() {
        let (rows, skipped, _) = parse_import_csv(
            "date,hours_worked\nnot-a-date,5\n2026-09-22,abc\n2026-09-23,4\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(skipped, 2);
        assert_eq!(rows[0].date, "2026-09-23");
    }

    #[test]
    fn auto_gap_sums_off_periods() {
        let log = "2026-10-01 20:42:54 -0500 Notification  \tDisplay is turned on\n\
                   2026-10-01 20:43:53 -0500 Notification  \tDisplay is turned off\n\
                   2026-10-01 20:44:13 -0500 Notification  \tDisplay is turned on\n\
                   2026-10-01 22:38:57 -0500 Notification  \tDisplay is turned off\n";
        assert_eq!(
            compute_auto_gap_minutes(log, "2026-10-01", 4),
            Some(0)
        );
        assert_eq!(compute_auto_gap_minutes(log, "2026-10-02", 4), None);
    }

    #[test]
    fn legacy_subtract_minutes_loads_as_manual() {
        let dir = std::env::temp_dir();
        let p = dir.join("wht_legacy_test.csv");
        std::fs::write(
            &p,
            "date,start,stop,subtract_minutes,hours_worked\n2026-10-01,20:42:54,22:38:57,15,1.68\n",
        )
        .unwrap();
        let map = load_entries_from_csv(p.to_str().unwrap());
        let e = &map["2026-10-01"];
        assert_eq!(e.subtract_manual_minutes, Some(15));
        assert_eq!(e.subtract_auto_minutes, None);
        let _ = std::fs::remove_file(&p);
    }

    fn tmp_csv(name: &str) -> String {
        let p = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&p);
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn epoch_zero_fills_pre_log_gaps() {
        // Log covers only 10-06..10-07. Epoch 10-01 -> 10-01..10-05 become 0h.
        let log = day_log("2026-10-06", "09:00:00", "17:00:00", None, "17:00:01")
            + &day_log("2026-10-07", "09:00:00", "12:00:00", None, "12:00:01");
        let path = tmp_csv("wht_epoch_test.csv");
        std::fs::write(&path, "date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked\n").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 7).unwrap();
        let rows = backfill_impl(&path, false, Some("2026-10-01"), &log, today, 4, 10).unwrap();
        let by: BTreeMap<String, &DayEntry> =
            rows.iter().map(|e| (e.date.clone(), e)).collect();
        for d in ["2026-10-01", "2026-10-02", "2026-10-03", "2026-10-04", "2026-10-05"] {
            let e = by[d];
            assert_eq!(e.hours_worked, 0.0);
            assert_eq!(e.start, "");
            assert_eq!(e.subtract_manual_minutes, Some(0));
            assert_eq!(e.subtract_auto_minutes, None);
        }
        assert_eq!(by["2026-10-06"].hours_worked, 8.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn no_epoch_leaves_pre_log_gaps_missing() {
        let log = day_log("2026-10-06", "09:00:00", "17:00:00", None, "17:00:01");
        let path = tmp_csv("wht_noepoch_test.csv");
        std::fs::write(&path, "date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked\n").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let rows = backfill_impl(&path, false, None, &log, today, 4, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "2026-10-06");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn epoch_does_not_touch_older_gaps() {
        // Epoch 10-05: 10-01..10-04 stay missing, 10-05 becomes 0h.
        let log = day_log("2026-10-06", "09:00:00", "17:00:00", None, "17:00:01");
        let path = tmp_csv("wht_epochbound_test.csv");
        std::fs::write(&path, "date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked\n").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let rows = backfill_impl(&path, false, Some("2026-10-05"), &log, today, 4, 10).unwrap();
        let dates: Vec<&str> = rows.iter().map(|e| e.date.as_str()).collect();
        assert_eq!(dates, vec!["2026-10-05", "2026-10-06"]);
        assert_eq!(rows[0].hours_worked, 0.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn plist_template_renders_paths() {
        let out = render_launchd_plist(
            Path::new("/Applications/Support/bin/wht-track"),
            Path::new("/tmp/wht-track.log"),
        );
        assert!(out.contains("/Applications/Support/bin/wht-track"));
        assert!(out.contains("/tmp/wht-track.log"));
        assert!(out.contains(launchd_label()));
        assert!(!out.contains("__BIN_PATH__") && !out.contains("__LOG_PATH__"));
    }

    #[test]
    fn consent_gate_detection() {
        let home = std::env::var("HOME").unwrap();
        assert!(csv_needs_consent(&format!("{}/Documents/work-hours.csv", home)));
        assert!(csv_needs_consent(&format!("{}/Desktop/x.csv", home)));
        assert!(csv_needs_consent(&format!("{}/Downloads/x.csv", home)));
        assert!(!csv_needs_consent(&default_csv_path()));
        assert!(!csv_needs_consent("/tmp/work-hours.csv"));
        // a sibling named e.g. ~/MyDocuments is NOT gated
        assert!(!csv_needs_consent(&format!("{}/MyDocuments/x.csv", home)));
    }

    #[test]
    fn default_csv_is_permission_free() {
        assert!(default_csv_path().contains("Application Support"));
    }

    #[test]
    fn settings_migrate_to_canonical_location() {
        // Sandbox HOME so default_csv_path() lands in a temp dir.
        let real_home = std::env::var("HOME").unwrap();
        let fake = std::env::temp_dir().join("wht_home_test");
        let _ = std::fs::remove_dir_all(&fake);
        std::fs::create_dir_all(&fake).unwrap();
        std::env::set_var("HOME", &fake);
        let def = default_csv_path();

        // Case 1: configured file exists, default missing -> moved, settings updated.
        let cfg_dir = fake.join("elsewhere");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_csv = cfg_dir.join("mine.csv");
        std::fs::write(&cfg_csv, "date\n2026-10-01\n").unwrap();
        let sp = app_support_dir().join("settings.json");
        std::fs::create_dir_all(app_support_dir()).unwrap();
        std::fs::write(
            &sp,
            format!(
                "{{\"csv_path\": \"{}\", \"use_auto_subtract\": true, \"background_epoch\": \"2026-10-01\"}}",
                cfg_csv.to_str().unwrap()
            ),
        )
        .unwrap();
        let st = load_settings_from(&sp);
        assert_eq!(st.csv_path, def);
        assert!(st.use_auto_subtract && st.background_epoch == Some("2026-10-01".to_string()));
        assert!(!cfg_csv.exists() && Path::new(&def).exists());

        // Case 2: both exist -> default wins, stray file untouched.
        std::fs::write(&cfg_csv, "date\n2026-10-02\n").unwrap();
        std::fs::write(
            &sp,
            format!("{{\"csv_path\": \"{}\"}}", cfg_csv.to_str().unwrap()),
        )
        .unwrap();
        let st2 = load_settings_from(&sp);
        assert_eq!(st2.csv_path, def);
        assert!(cfg_csv.exists());

        // Case 3: fresh (no settings) -> default.
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&def);
        let st3 = load_settings_from(&sp);
        assert_eq!(st3.csv_path, def);

        std::env::set_var("HOME", &real_home);
        let _ = std::fs::remove_dir_all(&fake);
    }

    #[test]
    fn overnight_hours_count_on_previous_logical_day() {
        let log = "2026-10-09 09:00:00 -0500 Notification  \tDisplay is turned on\n\
                   2026-10-10 02:00:00 -0500 Notification  \tDisplay is turned off\n";
        let (on, off) = parse_day(log, "2026-10-09", 4, 10, None);
        assert_eq!(on.as_deref(), Some("09:00:00"));
        assert_eq!(off.as_deref(), Some("02:00:00"));
        let (on10, off10) = parse_day(log, "2026-10-10", 4, 10, None);
        assert!(on10.is_none() && off10.is_none());
        assert_eq!(gross_hours("09:00:00", "02:00:00"), 17.0);
        assert_eq!(
            assign_logical_date(
                NaiveDate::from_ymd_opt(2026, 10, 10).unwrap(),
                parse_hms("02:00:00").unwrap(),
                4
            )
            .to_string(),
            "2026-10-09"
        );
    }

    #[test]
    fn short_display_blips_are_not_the_day_start() {
        let log = "2026-10-10 05:26:00 -0500 Notification  \tDisplay is turned on\n\
                   2026-10-10 05:26:20 -0500 Notification  \tDisplay is turned off\n\
                   2026-10-10 09:00:00 -0500 Notification  \tDisplay is turned on\n\
                   2026-10-10 17:00:00 -0500 Notification  \tDisplay is turned off\n";
        let (on, off) = parse_day(log, "2026-10-10", 4, 10, None);
        assert_eq!(on.as_deref(), Some("09:00:00"));
        assert_eq!(off.as_deref(), Some("17:00:00"));
    }
}
