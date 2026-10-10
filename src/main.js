import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";

let entries = [];
let today = null;
let settings = { csv_path: "", use_auto_subtract: false, use_24h: false, day_start_hour: 4, min_session_minutes: 10 };
let bucketSize = "1d";
let chartMetric = "hours";
// Custom range — same functionality as cursor-cost-tracking's RangePicker:
// preset buttons (to-date + rolling) plus a calendar for arbitrary ranges.
let customStart = "";
let customEnd = "";
let customPreset = "1mo";
let pickerOpen = false;
let draftFrom = null;
let draftTo = null;
let calCursor = null;

function parseLocalDate(s) {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s || "");
  if (!m) return null;
  const d = new Date(+m[1], +m[2] - 1, +m[3]);
  return isNaN(d) ? null : d;
}

function shiftDays(dateStr, delta) {
  const d = parseLocalDate(dateStr);
  if (!d) return dateStr;
  d.setDate(d.getDate() + delta);
  return fmtLocal(d);
}

function earliestEntryDate() {
  // entries arrive sorted ascending by date (BTreeMap on the backend)
  return entries.length ? entries[0].date : null;
}

const PRESET_LABELS = {
  today: "Today",
  wtd: "Week to date",
  mtd: "Month to date",
  ytd: "Year to date",
  "24h": "24 hours",
  "48h": "48 hours",
  "1w": "1 week",
  "2w": "2 weeks",
  "1mo": "1 month",
  "6mo": "6 months",
  "1y": "1 year",
  all: "All time",
};
const BUCKET_LABELS = { "1d": "1 day", "1w": "1 week", "1mo": "1 month" };

function logicalNow() {
  const d = new Date();
  const start = settings.day_start_hour ?? 4;
  if (d.getHours() < start) d.setDate(d.getDate() - 1);
  d.setHours(12, 0, 0, 0);
  return d;
}
function logicalTodayStr() {
  return today?.date || fmtLocal(logicalNow());
}

function applyPresetRange(id) {
  const now = logicalNow();
  const todayStr = fmtLocal(now);
  let s = todayStr;
  if (id === "wtd") s = fmtLocal(mondayOf(now));
  else if (id === "mtd") s = fmtLocal(new Date(now.getFullYear(), now.getMonth(), 1));
  else if (id === "ytd") s = fmtLocal(new Date(now.getFullYear(), 0, 1));
  else if (id === "24h") s = todayStr;
  else if (id === "48h") s = shiftDays(todayStr, -1);
  else if (id === "1w") s = shiftDays(todayStr, -6);
  else if (id === "2w") s = shiftDays(todayStr, -13);
  else if (id === "1mo") s = shiftDays(todayStr, -29);
  else if (id === "6mo") s = shiftDays(todayStr, -179);
  else if (id === "1y") s = shiftDays(todayStr, -364);
  else if (id === "all") s = earliestEntryDate() || todayStr;
  customStart = s;
  customEnd = todayStr;
  customPreset = id;
}
applyPresetRange("1mo");

function fmtRangeLabel() {
  if (customPreset && PRESET_LABELS[customPreset]) return PRESET_LABELS[customPreset];
  const s = parseLocalDate(customStart);
  const e = parseLocalDate(customEnd);
  if (!s || !e) return "Custom";
  const curYear = new Date().getFullYear();
  const opts = { month: "short", day: "numeric" };
  if (s.getFullYear() !== curYear || e.getFullYear() !== curYear) opts.year = "numeric";
  return `${s.toLocaleDateString("en-US", opts)} – ${e.toLocaleDateString("en-US", opts)}`;
}

const $ = (id) => document.getElementById(id);

function mondayOf(d) {
  const x = new Date(d);
  const day = (x.getDay() + 6) % 7; // Mon=0
  x.setDate(x.getDate() - day);
  x.setHours(0, 0, 0, 0);
  return x;
}
function fmtLocal(d) {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}
function fmtH(h) {
  return h == null ? "n/a" : h.toFixed(2);
}
function fmtSub(v) {
  return v == null ? "n/a" : String(v);
}
function hoursFor(e, key) {
  if (today && key === today.date) return today.hours_so_far ?? 0;
  return e ? e.hours_worked ?? 0 : 0;
}
function timesFor(e, key) {
  if (today && key === today.date) {
    return { start: today.start || "", stop: today.stop_or_now || "" };
  }
  return { start: (e && e.start) || "", stop: (e && e.stop) || "" };
}
function mean(arr) {
  return arr.length ? arr.reduce((a, b) => a + b, 0) / arr.length : null;
}
function summarizeKeys(keys, m, todayStr) {
  let total = 0, count = 0;
  const starts = [];
  const stops = [];
  for (const key of keys) {
    if (key > todayStr) continue;
    const e = m.get(key);
    if (!e) continue;
    const hrs = hoursFor(e, key);
    total += hrs;
    count++;
    if (hrs > 0) {
      const t = timesFor(e, key);
      const ss = timeToDaySec(t.start);
      const es = timeToDaySec(t.stop);
      if (ss != null) starts.push(ss);
      if (es != null) stops.push(es);
    }
  }
  const startSec = mean(starts);
  const stopSec = mean(stops);
  return {
    hours: count ? Math.round(total * 100) / 100 : null,
    startSec,
    stopSec,
    start: startSec != null ? daySecToClock(startSec) : "",
    stop: stopSec != null ? daySecToClock(stopSec) : "",
    count,
  };
}
function timeToSec(t) {
  if (!t) return null;
  const p = String(t).split(":").map(Number);
  if (p.length < 2 || p.some((n) => Number.isNaN(n))) return null;
  return p[0] * 3600 + p[1] * 60 + (p[2] || 0);
}
function secToHm(sec) {
  const s = ((Math.round(sec) % 86400) + 86400) % 86400;
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}
function timeToDaySec(t) {
  const sec = timeToSec(t);
  if (sec == null) return null;
  const start = (settings.day_start_hour ?? 4) * 3600;
  return sec >= start ? sec - start : sec + 86400 - start;
}
function daySecToClock(sec) {
  return secToHm(sec + (settings.day_start_hour ?? 4) * 3600);
}
function fmtClock(t) {
  if (!t || t === "—" || t === "running…") return t || "—";
  const sec = timeToSec(t);
  if (sec == null) return t;
  const h24 = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const mm = String(m).padStart(2, "0");
  if (settings.use_24h) return `${String(h24).padStart(2, "0")}:${mm}`;
  const ap = h24 >= 12 ? "PM" : "AM";
  let h = h24 % 12;
  if (h === 0) h = 12;
  return `${h}:${mm} ${ap}`;
}

async function load() {
  try {
    entries = await invoke("get_entries");
    today = await invoke("get_today");
    settings = await invoke("get_settings");
  } catch (e) {
    console.error(e);
    return;
  }
  renderToday();
  renderSettings();
  renderAll();
}

function byDate() {
  const m = new Map();
  for (const e of entries) m.set(e.date, e);
  return m;
}

function selectedRange() {
  const todayStr = logicalTodayStr();
  const sStr = customPreset === "all" ? earliestEntryDate() || todayStr : customStart;
  let s = parseLocalDate(sStr);
  let e = parseLocalDate(customEnd);
  if (!s || !e || s > e) {
    s = parseLocalDate(todayStr);
    e = s;
  }
  return { start: fmtLocal(s), end: fmtLocal(e), s, e };
}

function dayItem(d, m, todayStr) {
  const key = fmtLocal(d);
  const e = m.get(key);
  if (key > todayStr) {
    return { label: "", sub: "", key, hours: null, start: "", stop: "", startSec: null, stopSec: null, na: false, future: true, date: d };
  }
  if (!e) {
    return { label: "", sub: key.slice(5), key, hours: null, start: "", stop: "", startSec: null, stopSec: null, na: true, future: false, date: d };
  }
  const t = timesFor(e, key);
  return {
    label: "", sub: key.slice(5), key, hours: hoursFor(e, key),
    start: t.start, stop: t.stop, startSec: timeToDaySec(t.start), stopSec: timeToDaySec(t.stop),
    na: false, future: false, date: d,
  };
}

function groupWeeks(startStr, endStr, m, todayStr) {
  const s = parseLocalDate(startStr);
  const e = parseLocalDate(endStr);
  const out = [];
  if (!s || !e) return out;
  const cur = mondayOf(s);
  while (cur <= e) {
    const keys = [];
    for (let k = 0; k < 7; k++) {
      const d = new Date(cur);
      d.setDate(cur.getDate() + k);
      const key = fmtLocal(d);
      if (key < startStr || key > endStr) continue;
      keys.push(key);
    }
    const s = summarizeKeys(keys, m, todayStr);
    const monStr = fmtLocal(cur);
    const future = monStr > todayStr;
    out.push({
      label: `Wk ${monStr.slice(5)}`, sub: "", key: monStr,
      hours: s.hours, start: s.start, stop: s.stop, startSec: s.startSec, stopSec: s.stopSec,
      na: !s.count && !future, future, grouped: true,
    });
    cur.setDate(cur.getDate() + 7);
  }
  return out;
}

function groupMonths(startStr, endStr, m, todayStr) {
  const s = parseLocalDate(startStr);
  const e = parseLocalDate(endStr);
  const out = [];
  if (!s || !e) return out;
  const cur = new Date(s.getFullYear(), s.getMonth(), 1);
  while (cur <= e) {
    const y = cur.getFullYear(), mo = cur.getMonth();
    const n = new Date(y, mo + 1, 0).getDate();
    const keys = [];
    for (let k = 1; k <= n; k++) {
      const key = `${y}-${String(mo + 1).padStart(2, "0")}-${String(k).padStart(2, "0")}`;
      if (key >= startStr && key <= endStr) keys.push(key);
    }
    const sum = summarizeKeys(keys, m, todayStr);
    const d = new Date(y, mo, 1);
    const future = fmtLocal(d) > todayStr;
    out.push({
      label: d.toLocaleString("en-US", { month: "short" }),
      sub: String(y).slice(2),
      key: `${y}-${String(mo + 1).padStart(2, "0")}`,
      hours: sum.hours, start: sum.start, stop: sum.stop, startSec: sum.startSec, stopSec: sum.stopSec,
      na: !sum.count && !future, future, grouped: true,
    });
    cur.setMonth(cur.getMonth() + 1);
  }
  return out;
}

function renderToday() {
  if (!today) return;
  $("todayDate").textContent = today.date;
  $("todayRange").textContent = today.start ? `${fmtClock(today.start)} → ${fmtClock(today.stop_or_now) || "…"}` : "not started yet";
  $("todayHours").textContent = (today.hours_so_far ?? 0).toFixed(2) + "h";
  $("todayLive").style.display = today.is_running ? "block" : "none";
  $("todayAuto").textContent = today.subtract_auto_minutes == null ? "n/a" : `${today.subtract_auto_minutes} min`;
  const inp = $("todaySubtract");
  if (document.activeElement !== inp) inp.value = today.subtract_manual_minutes ?? 0;
  $("modeBadge").textContent = settings.use_auto_subtract ? "stats: auto" : "stats: manual";
}

function hourLabel(h) {
  if (settings.use_24h) return `${String(h).padStart(2, "0")}:00`;
  const ap = h >= 12 ? "PM" : "AM";
  let x = h % 12;
  if (x === 0) x = 12;
  return `${x}:00 ${ap}`;
}

function renderSettings() {
  $("csvPath").textContent = settings.csv_path || "—";
  $("useAutoToggle").checked = !!settings.use_auto_subtract;
  $("use24hToggle").checked = !!settings.use_24h;
  const sel = $("dayStartHour");
  const cur = settings.day_start_hour ?? 4;
  sel.innerHTML = "";
  for (let h = 0; h < 24; h++) {
    const o = document.createElement("option");
    o.value = String(h);
    o.textContent = hourLabel(h);
    if (h === cur) o.selected = true;
    sel.append(o);
  }
  const minInp = $("minSessionMinutes");
  if (document.activeElement !== minInp) minInp.value = settings.min_session_minutes ?? 10;
  $("bgMsg").textContent = "";
  refreshBgStatus();
}

let bgOn = false;

async function refreshBgStatus() {
  try {
    const s = await invoke("background_status");
    bgOn = s.installed && s.loaded;
    $("bgToggleBtn").textContent = bgOn ? "Disable hourly tracking" : "Enable hourly tracking";
    const parts = [];
    if (s.epoch) parts.push(`active since ${s.epoch}`);
    parts.push(s.installed ? (s.loaded ? "job scheduled ✓" : "job installed but not loaded") : "job not installed");
    if (!s.helper_available) parts.push("helper binary not bundled (dev: run scripts/bundle-helper.sh)");
    if (s.csv_protected)
      parts.push("WARNING: CSV is in a gated folder — move it out so the background job can reach it.");
    $("bgStatus").textContent = parts.join(" · ");
    $("bgLastRun").textContent = s.last_line ? `Last background run: ${s.last_line}` : "No background runs logged yet.";
  } catch (e) {
    $("bgMsg").textContent = `Background status failed: ${e}`;
  }
}

$("bgToggleBtn").addEventListener("click", async () => {
  $("bgMsg").textContent = bgOn ? "Disabling…" : "Installing… (copies helper, registers hourly job, test-runs it)";
  try {
    const s = await invoke(bgOn ? "uninstall_background_job" : "install_background_job");
    bgOn = s.installed && s.loaded;
    $("bgToggleBtn").textContent = bgOn ? "Disable hourly tracking" : "Enable hourly tracking";
    $("bgMsg").textContent = bgOn ? "Hourly tracking enabled — runs at :05 past each hour." : "Hourly tracking disabled.";
    await refreshBgStatus();
    await load();
  } catch (e) {
    $("bgMsg").textContent = `Failed: ${e}`;
  }
});

function eachDate(start, end, todayStr) {
  const out = [];
  const s = new Date(start);
  const e = new Date(end);
  for (let d = new Date(s); d <= e; d.setDate(d.getDate() + 1)) {
    const key = fmtLocal(d);
    if (key > todayStr) continue;
    out.push(key);
  }
  return out;
}

function viewDayKeys() {
  const todayStr = logicalTodayStr();
  const r = selectedRange();
  return eachDate(r.s, r.e, todayStr);
}

function barData() {
  const m = byDate();
  const todayStr = logicalTodayStr();
  const r = selectedRange();
  const title = `${fmtRangeLabel()} · ${BUCKET_LABELS[bucketSize] || "1 day"}`;
  if (bucketSize === "1w") {
    return { title, items: groupWeeks(r.start, r.end, m, todayStr), grouped: true };
  }
  if (bucketSize === "1mo") {
    return { title, items: groupMonths(r.start, r.end, m, todayStr), grouped: true };
  }
  const days = Math.round((r.e - r.s) / 86400000) + 1;
  const items = [];
  for (let i = 0; i < days; i++) {
    const d = new Date(r.s);
    d.setDate(r.s.getDate() + i);
    const it = dayItem(d, m, todayStr);
    it.label = days <= 7 ? d.toLocaleDateString("en-US", { weekday: "short" }) : fmtLocal(d).slice(5);
    it.sub = days <= 7 ? fmtLocal(d).slice(5) : "";
    items.push(it);
  }
  return { title, items, grouped: false };
}

function metricVal(it) {
  if (chartMetric === "start") return it.startSec;
  if (chartMetric === "stop") return it.stopSec;
  return it.hours;
}
function metricText(it) {
  if (it.future) return "";
  if (chartMetric === "start" || chartMetric === "stop") {
    const t = chartMetric === "start" ? it.start : it.stop;
    if (it.na || t == null || t === "") return "n/a";
    return fmtClock(t);
  }
  if (it.na || it.hours == null) return "n/a";
  return it.hours ? it.hours.toFixed(it.hours < 10 ? 2 : 1) : "0";
}
function hideTip() {
  $("chartTip").classList.add("hidden");
}
function placeTip(ev) {
  const tip = $("chartTip");
  const pad = 12;
  let x = ev.clientX + 14;
  let y = ev.clientY + 14;
  const tw = tip.offsetWidth;
  const th = tip.offsetHeight;
  if (x + tw + pad > window.innerWidth) x = ev.clientX - tw - 10;
  if (y + th + pad > window.innerHeight) y = ev.clientY - th - 10;
  tip.style.left = `${x}px`;
  tip.style.top = `${y}px`;
}
function showTip(it, ev, grouped) {
  const tip = $("chartTip");
  const hours = it.future || it.na || it.hours == null ? "n/a" : `${it.hours.toFixed(2)}h`;
  const start = it.future || !it.start ? "n/a" : fmtClock(it.start);
  const stop = it.future || !it.stop ? "n/a" : fmtClock(it.stop);
  const label = it.key && it.label && it.key !== it.label ? `${it.label} · ${it.key}` : (it.label || it.key || "");
  const startL = grouped ? "Avg start" : "Start";
  const stopL = grouped ? "Avg stop" : "Stop";
  tip.innerHTML = `<div class="chart-tooltip-label">${label}</div>
    <div class="chart-tooltip-row"><span>Hours</span><strong>${hours}</strong></div>
    <div class="chart-tooltip-row"><span>${startL}</span><strong>${start}</strong></div>
    <div class="chart-tooltip-row"><span>${stopL}</span><strong>${stop}</strong></div>`;
  tip.classList.remove("hidden");
  placeTip(ev);
}

function renderAll() {
  hideTip();
  const { title, items, grouped } = barData();
  const groupedTime = grouped && chartMetric !== "hours";
  const metricName = chartMetric === "hours" ? "hours" : groupedTime ? `avg ${chartMetric}` : chartMetric;
  $("viewTitle").textContent = `${title} · ${metricName} · ${settings.use_auto_subtract ? "auto subtract" : "manual subtract"}`;
  $("rangeLabel").textContent = fmtRangeLabel();
  const vals = items.filter((i) => !i.future && !i.na && metricVal(i) != null).map(metricVal);
  const max = Math.max(1, ...vals);
  const chart = $("chart");
  chart.innerHTML = "";
  for (const it of items) {
    const col = document.createElement("div");
    col.className = "bar-col";
    const val = metricVal(it);
    const empty = it.future || it.na || val == null;
    const v = document.createElement("div");
    v.className = "bar-val" + (empty && !it.future ? " na" : "");
    v.textContent = metricText(it);
    const b = document.createElement("div");
    const h = empty ? 8 : Math.max(10, Math.round((val / max) * 170));
    const isZero = chartMetric === "hours" && !it.hours && !it.na && !it.future;
    b.className = "bar" + (isZero ? " zero" : "") + (it.future ? " future" : "") + (empty && !it.future ? " na" : "");
    b.style.height = h + "px";
    const l = document.createElement("div");
    l.className = "bar-label";
    l.textContent = it.label;
    col.append(v, b, l);
    if (it.sub) {
      const s = document.createElement("div");
      s.className = "bar-sub";
      s.textContent = it.sub;
      col.append(s);
    }
    col.addEventListener("pointerenter", (ev) => showTip(it, ev, grouped));
    col.addEventListener("pointermove", placeTip);
    col.addEventListener("pointerleave", hideTip);
    chart.append(col);
  }
  const n = items.length;
  chart.style.gap = n > 50 ? "1px" : n > 30 ? "2px" : n > 16 ? "4px" : n > 10 ? "6px" : "10px";
  requestAnimationFrame(() => {
    for (const col of chart.querySelectorAll(".bar-col")) {
      const w = col.clientWidth;
      const val = col.querySelector(".bar-val");
      const lab = col.querySelector(".bar-label");
      const sub = col.querySelector(".bar-sub");
      if (val) val.classList.toggle("tight", w < (chartMetric === "hours" ? 28 : 44));
      if (lab) lab.classList.toggle("tight", w < 18);
      if (sub) sub.classList.toggle("tight", w < 22);
    }
  });
  const m = byDate();
  const todayStr = logicalTodayStr();
  const dayKeys = viewDayKeys();
  const allDays = [];
  const worked = [];
  for (const key of dayKeys) {
    const e = m.get(key);
    if (!e) continue;
    const hours = hoursFor(e, key);
    allDays.push({ key, e, hours });
    if (hours > 0) worked.push({ key, e, hours });
  }
  const workedExcl = worked.filter((d) => d.key !== todayStr);
  const meanH = (arr) => (arr.length ? (arr.reduce((a, b) => a + b.hours, 0) / arr.length).toFixed(2) + "h" : "n/a");
  const total = allDays.reduce((a, b) => a + b.hours, 0);
  $("totalH").textContent = total.toFixed(2) + "h";
  $("avgH").textContent = allDays.length ? (total / allDays.length).toFixed(2) + "h" : "n/a";
  $("avgWorkedH").textContent = meanH(worked);
  $("avgWorkedExclH").textContent = meanH(workedExcl);
  const starts = worked.map((d) => timeToDaySec(d.e.start)).filter((x) => x != null);
  const stops = worked.map((d) => timeToDaySec(d.e.stop)).filter((x) => x != null);
  $("avgStart").textContent = starts.length
    ? fmtClock(daySecToClock(starts.reduce((a, b) => a + b, 0) / starts.length))
    : "n/a";
  $("avgStop").textContent = stops.length
    ? fmtClock(daySecToClock(stops.reduce((a, b) => a + b, 0) / stops.length))
    : "n/a";
  $("daysN").textContent = String(allDays.length);
  const tb = $("rows");
  tb.innerHTML = "";
  if (!grouped) {
    const rows = items.filter((i) => !i.future).slice().reverse();
    for (const it of rows) {
      const e = byDate().get(it.key);
      if (e) tb.append(rowEl(e));
      else {
        const tr = document.createElement("tr");
        tr.className = "na-row";
        tr.innerHTML = `<td>${it.key}</td><td colspan="3">no data (n/a)</td><td>n/a</td>`;
        tb.append(tr);
      }
    }
  } else {
    for (const it of items.slice().reverse()) {
      const tr = document.createElement("tr");
      const hrs = it.future || it.na ? "n/a" : it.hours.toFixed(2);
      tr.innerHTML = `<td>${it.key}</td><td colspan="2">${it.label}</td><td>—</td><td>—</td><td>${hrs}</td>`;
      tb.append(tr);
    }
  }
}

function rowEl(e) {
  const tr = document.createElement("tr");
  const start = e.start ? fmtClock(e.start) : "—";
  const stop = e.stop ? fmtClock(e.stop) : (e.date === today?.date && today?.is_running ? "running…" : "—");
  const td0 = document.createElement("td");
  td0.textContent = e.date;
  const td1 = document.createElement("td");
  td1.textContent = start;
  const td2 = document.createElement("td");
  td2.textContent = stop;
  tr.append(td0, td1, td2);
  const td = document.createElement("td");
  const inp = document.createElement("input");
  inp.type = "number"; inp.min = "0"; inp.max = "1440";
  inp.value = e.subtract_manual_minutes ?? "";
  inp.placeholder = "n/a";
  inp.addEventListener("change", async () => {
    const raw = inp.value === "" ? 0 : parseInt(inp.value || "0", 10);
    const mins = Math.max(0, Math.min(1440, raw));
    inp.value = mins;
    try {
      await invoke("set_subtract", { date: e.date, minutes: mins });
      $("saveMsg").textContent = `Saved ${e.date}: manual -${mins} min`;
      setTimeout(() => ($("saveMsg").textContent = ""), 2000);
      await load();
    } catch (err) {
      console.error(err);
    }
  });
  td.append(inp);
  const tda = document.createElement("td");
  tda.textContent = fmtSub(e.subtract_auto_minutes);
  tda.className = e.subtract_auto_minutes == null ? "na" : "";
  const htd = document.createElement("td");
  htd.textContent = fmtH(hoursFor(e, e.date));
  tr.append(td, tda, htd);
  return tr;
}

$("bucketSize").addEventListener("change", (ev) => {
  bucketSize = ev.target.value;
  renderAll();
});
$("chartMetric").addEventListener("change", (ev) => {
  chartMetric = ev.target.value;
  renderAll();
});

// ---- RangePicker panel (ported from cursor-cost-tracking) ----
function markActivePreset() {
  document.querySelectorAll("#rangePanel .preset").forEach((b) => {
    b.classList.toggle("active", b.dataset.preset === customPreset);
  });
}

function setPickerOpen(o) {
  pickerOpen = o;
  $("rangePanel").classList.toggle("hidden", !o);
  if (o) {
    draftFrom = customStart || null;
    draftTo = customEnd && customEnd !== customStart ? customEnd : null;
    const base = parseLocalDate(draftTo || draftFrom) || new Date();
    calCursor = { y: base.getFullYear(), m: base.getMonth() };
    $("rangeMsg").textContent = "";
    markActivePreset();
    renderCalendar();
  }
}

function renderCalendar() {
  const todayStr = logicalTodayStr();
  $("calTitle").textContent = new Date(calCursor.y, calCursor.m, 1).toLocaleDateString("en-US", {
    month: "long",
    year: "numeric",
  });
  const grid = $("calGrid");
  grid.innerHTML = "";
  ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"].forEach((w) => {
    const el = document.createElement("div");
    el.className = "cal-weekday";
    el.textContent = w;
    grid.append(el);
  });
  const first = new Date(calCursor.y, calCursor.m, 1);
  const lead = (first.getDay() + 6) % 7;
  for (let i = 0; i < lead; i++) {
    const el = document.createElement("div");
    el.className = "cal-day blank";
    grid.append(el);
  }
  const days = new Date(calCursor.y, calCursor.m + 1, 0).getDate();
  for (let d = 1; d <= days; d++) {
    const key = fmtLocal(new Date(calCursor.y, calCursor.m, d));
    const el = document.createElement("button");
    el.type = "button";
    el.textContent = String(d);
    const disabled = key > todayStr;
    el.className = "cal-day" + (disabled ? " disabled" : "");
    if (!disabled) {
      if (key === draftFrom || (draftTo && key === draftTo)) el.classList.add("sel");
      else if (draftFrom && draftTo && key > draftFrom && key < draftTo) el.classList.add("in-range");
      if (key === todayStr) el.classList.add("today");
      el.addEventListener("click", () => {
        if (!draftFrom || (draftFrom && draftTo)) {
          draftFrom = key;
          draftTo = null;
        } else if (key >= draftFrom) {
          draftTo = key;
        } else {
          draftFrom = key;
        }
        renderCalendar();
      });
    } else {
      el.disabled = true;
    }
    grid.append(el);
  }
  const atCurrentMonth =
    calCursor.y === new Date().getFullYear() && calCursor.m === new Date().getMonth();
  $("calNext").disabled = atCurrentMonth;
}

$("calPrev").addEventListener("click", () => {
  calCursor.m--;
  if (calCursor.m < 0) {
    calCursor.m = 11;
    calCursor.y--;
  }
  renderCalendar();
});
$("calNext").addEventListener("click", () => {
  if ($("calNext").disabled) return;
  calCursor.m++;
  if (calCursor.m > 11) {
    calCursor.m = 0;
    calCursor.y++;
  }
  renderCalendar();
});

document.querySelectorAll("#rangePanel .preset").forEach((b) =>
  b.addEventListener("click", () => {
    applyPresetRange(b.dataset.preset);
    setPickerOpen(false);
    renderAll();
  })
);

$("rangeButton").addEventListener("click", (e) => {
  e.stopPropagation();
  setPickerOpen(!pickerOpen);
});
$("rangeCancel").addEventListener("click", () => setPickerOpen(false));
$("rangeApply").addEventListener("click", () => {
  if (!draftFrom) return;
  customStart = draftFrom;
  customEnd = draftTo || draftFrom;
  customPreset = null;
  setPickerOpen(false);
  renderAll();
});
document.addEventListener("mousedown", (e) => {
  if (pickerOpen && !$("pickerWrap").contains(e.target)) setPickerOpen(false);
});

$("todaySubtract").addEventListener("change", async (ev) => {
  if (!today) return;
  const mins = Math.max(0, Math.min(1440, parseInt(ev.target.value || "0", 10)));
  ev.target.value = mins;
  try {
    await invoke("set_subtract", { date: today.date, minutes: mins });
    $("saveMsg").textContent = `Saved today: manual -${mins} min`;
    setTimeout(() => ($("saveMsg").textContent = ""), 2000);
    await load();
  } catch (e) {
    console.error(e);
  }
});

$("minSessionMinutes").addEventListener("change", async (ev) => {
  try {
    const minutes = Math.max(0, Math.min(240, Number(ev.target.value) || 0));
    ev.target.value = minutes;
    entries = await invoke("set_min_session_minutes", { minutes });
    settings.min_session_minutes = minutes;
    today = await invoke("get_today");
    settings = await invoke("get_settings");
    renderToday();
    renderSettings();
    renderAll();
  } catch (e) {
    console.error(e);
  }
});

$("dayStartHour").addEventListener("change", async (ev) => {
  try {
    entries = await invoke("set_day_start_hour", { hour: Number(ev.target.value) });
    settings.day_start_hour = Number(ev.target.value);
    today = await invoke("get_today");
    settings = await invoke("get_settings");
    renderToday();
    renderSettings();
    renderAll();
  } catch (e) {
    console.error(e);
  }
});

$("use24hToggle").addEventListener("change", async (ev) => {
  try {
    settings = await invoke("set_use_24h", { use24h: ev.target.checked });
    renderToday();
    renderSettings();
    renderAll();
  } catch (e) {
    console.error(e);
  }
});

$("useAutoToggle").addEventListener("change", async (ev) => {
  try {
    entries = await invoke("set_use_auto", { useAuto: ev.target.checked });
    settings.use_auto_subtract = ev.target.checked;
    today = await invoke("get_today");
    renderToday();
    renderAll();
  } catch (e) {
    console.error(e);
  }
});

$("refreshBtn").addEventListener("click", async () => {
  try {
    entries = await invoke("refresh");
    today = await invoke("get_today");
    settings = await invoke("get_settings");
    renderToday();
    renderAll();
  } catch (e) {
    console.error(e);
  }
});

$("settingsBtn").addEventListener("click", () => {
  renderSettings();
  $("importMsg").textContent = "";
  $("settingsModal").classList.remove("hidden");
});
function closeSettings() {
  $("settingsModal").classList.add("hidden");
}
$("closeSettings").addEventListener("click", closeSettings);
$("settingsModal").addEventListener("click", (e) => {
  if (e.target.id === "settingsModal") closeSettings();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !$("settingsModal").classList.contains("hidden")) closeSettings();
});

$("uploadBtn").addEventListener("click", async () => {
  const sel = await open({ multiple: false, directory: false, filters: [{ name: "CSV", extensions: ["csv"] }] });
  const path = typeof sel === "string" ? sel : null;
  if (!path) return;
  $("importMsg").textContent = "Importing…";
  try {
    const res = await invoke("import_backfill_csv", { filePath: path });
    const errs = res.errors.length ? ` (${res.errors.length} skipped: ${res.errors.slice(0, 3).join("; ")}${res.errors.length > 3 ? "…" : ""})` : "";
    $("importMsg").textContent = `Done: ${res.imported} new, ${res.updated} updated, ${res.skipped} skipped.${errs}`;
    await load();
  } catch (e) {
    $("importMsg").textContent = `Import failed: ${e}`;
  }
});

setInterval(load, 60000);
window.addEventListener("focus", load);

// ---- In-app updates (same UX as cursor-cost-tracking) ----
let pendingUpdate = null;
let checkingUpdate = false;
let installingUpdate = false;

function downloadPercent(downloaded, total) {
  if (!total || total <= 0) return null;
  return Math.min(100, Math.round((downloaded / total) * 100));
}

async function checkUpdates(fromSettings) {
  if (checkingUpdate) return;
  checkingUpdate = true;
  if (fromSettings) {
    $("updateMsg").textContent = "";
    $("checkUpdateBtn").textContent = "Checking…";
  }
  try {
    const update = await check();
    if (update) {
      if (!installingUpdate && (!pendingUpdate || pendingUpdate.version !== update.version)) {
        pendingUpdate = update;
        showToast(update.version, null, null);
      }
      if (fromSettings) $("updateMsg").textContent = `Version ${update.version} is available.`;
    } else if (fromSettings) {
      const v = await getVersion().catch(() => "current");
      $("updateMsg").textContent = `You are on the latest version (${v}).`;
    }
  } catch (e) {
    if (fromSettings) $("updateMsg").textContent = String(e?.message || e);
  } finally {
    checkingUpdate = false;
    if (fromSettings) $("checkUpdateBtn").textContent = "Check for updates";
  }
}

function showToast(version, percent, error) {
  $("updateToast").classList.remove("hidden");
  $("updateToastText").textContent = `Version ${version} is ready. Restart and update now?`;
  const p = $("updateToastProgress");
  if (percent === null || percent === undefined) p.classList.add("hidden");
  else {
    p.classList.remove("hidden");
    p.textContent = percent === null ? "Downloading update…" : `Downloading update… ${percent}%`;
  }
  const er = $("updateToastError");
  if (error) {
    er.classList.remove("hidden");
    er.textContent = error;
  } else er.classList.add("hidden");
  $("updateInstallBtn").textContent = installingUpdate ? "Updating…" : "Restart and update";
  $("updateInstallBtn").disabled = installingUpdate;
  $("updateLaterBtn").disabled = installingUpdate;
}

$("updateLaterBtn").addEventListener("click", () => {
  if (installingUpdate) return;
  $("updateToast").classList.add("hidden");
});

$("updateInstallBtn").addEventListener("click", async () => {
  if (!pendingUpdate || installingUpdate) return;
  installingUpdate = true;
  showToast(pendingUpdate.version, 0, null);
  let downloaded = 0;
  let total;
  try {
    await pendingUpdate.downloadAndInstall((event) => {
      if (event.event === "Started") {
        downloaded = 0;
        total = event.data.contentLength;
        showToast(pendingUpdate.version, downloadPercent(0, total), null);
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        showToast(pendingUpdate.version, downloadPercent(downloaded, total), null);
      } else if (event.event === "Finished") {
        showToast(pendingUpdate.version, 100, null);
      }
    });
    await relaunch();
  } catch (e) {
    installingUpdate = false;
    showToast(pendingUpdate.version, null, String(e?.message || e));
  }
});

$("checkUpdateBtn").addEventListener("click", () => checkUpdates(true));

getVersion()
  .then((v) => ($("appVersion").textContent = v))
  .catch(() => ($("appVersion").textContent = "…"));
checkUpdates(false);
setInterval(() => checkUpdates(false), 60 * 60 * 1000);

load();
