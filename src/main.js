import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

let entries = [];
let today = null;
let settings = { csv_path: "", use_auto_subtract: false };
let view = "week";
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
  "1w": "Last 7 days",
  "2w": "Last 14 days",
  "1mo": "Last 30 days",
  "6mo": "Last 6 months",
  "1y": "Last year",
  all: "All time",
};

function applyPresetRange(id) {
  const now = new Date();
  const todayStr = fmtLocal(now);
  let s = todayStr;
  if (id === "wtd") s = fmtLocal(mondayOf(now));
  else if (id === "mtd") s = fmtLocal(new Date(now.getFullYear(), now.getMonth(), 1));
  else if (id === "ytd") s = fmtLocal(new Date(now.getFullYear(), 0, 1));
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

function weekDays() {
  const mon = mondayOf(new Date());
  const out = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(mon);
    d.setDate(mon.getDate() + i);
    out.push(d);
  }
  return out;
}

function monthDays() {
  const now = new Date();
  const out = [];
  const y = now.getFullYear(), m = now.getMonth();
  const n = new Date(y, m + 1, 0).getDate();
  for (let i = 1; i <= n; i++) out.push(new Date(y, m, i));
  return out;
}

// A day item: {label, sub, key, hours|null, na, future}
function dayItem(d, m, todayStr) {
  const key = fmtLocal(d);
  const e = m.get(key);
  if (key > todayStr) return { label: "", sub: "", key, hours: null, na: false, future: true, date: d };
  if (!e) return { label: "", sub: key.slice(5), key, hours: null, na: true, future: false, date: d };
  return { label: "", sub: key.slice(5), key, hours: e.hours_worked ?? 0, na: false, future: false, date: d };
}

function lastNWeeks(n) {
  const m = byDate();
  const todayStr = fmtLocal(new Date());
  const cur = mondayOf(new Date());
  const out = [];
  for (let i = n - 1; i >= 0; i--) {
    const mon = new Date(cur);
    mon.setDate(cur.getDate() - i * 7);
    let total = 0, count = 0;
    for (let k = 0; k < 7; k++) {
      const d = new Date(mon);
      d.setDate(mon.getDate() + k);
      const key = fmtLocal(d);
      if (key > todayStr) continue;
      const e = m.get(key);
      if (e) { total += e.hours_worked || 0; count++; }
    }
    const future = fmtLocal(mon) > todayStr;
    out.push({
      label: `Wk ${fmtLocal(mon).slice(5)}`, sub: "", key: fmtLocal(mon),
      hours: count ? Math.round(total * 100) / 100 : null,
      na: !count && !future, future,
    });
  }
  return out;
}

// Group an arbitrary [start,end] date span into Mon–Sun weekly buckets.
function groupWeeks(startStr, endStr, m, todayStr) {
  const s = parseLocalDate(startStr);
  const e = parseLocalDate(endStr);
  const out = [];
  if (!s || !e) return out;
  const cur = mondayOf(s);
  while (cur <= e) {
    let total = 0, count = 0;
    for (let k = 0; k < 7; k++) {
      const d = new Date(cur);
      d.setDate(cur.getDate() + k);
      const key = fmtLocal(d);
      if (key < startStr || key > endStr || key > todayStr) continue;
      const en = m.get(key);
      if (en) { total += en.hours_worked || 0; count++; }
    }
    const monStr = fmtLocal(cur);
    const future = monStr > todayStr;
    out.push({
      label: `Wk ${monStr.slice(5)}`, sub: "", key: monStr,
      hours: count ? Math.round(total * 100) / 100 : null,
      na: !count && !future, future, grouped: true,
    });
    cur.setDate(cur.getDate() + 7);
  }
  return out;
}

function lastNMonths(n) {
  const m = byDate();
  const todayStr = fmtLocal(new Date());
  const now = new Date();
  const out = [];
  for (let i = n - 1; i >= 0; i--) {
    const d = new Date(now.getFullYear(), now.getMonth() - i, 1);
    const y = d.getFullYear(), mo = d.getMonth();
    const days = new Date(y, mo + 1, 0).getDate();
    let total = 0, count = 0;
    for (let k = 1; k <= days; k++) {
      const key = `${y}-${String(mo + 1).padStart(2, "0")}-${String(k).padStart(2, "0")}`;
      if (key > todayStr) continue;
      const e = m.get(key);
      if (e) { total += e.hours_worked || 0; count++; }
    }
    const future = fmtLocal(d) > todayStr;
    const label = d.toLocaleString("en-US", { month: "short" });
    out.push({
      label, sub: String(y).slice(2), key: `${y}-${String(mo + 1).padStart(2, "0")}`,
      hours: count ? Math.round(total * 100) / 100 : null,
      na: !count && !future, future,
    });
  }
  return out;
}

function renderToday() {
  if (!today) return;
  $("todayDate").textContent = today.date;
  $("todayRange").textContent = today.start ? `${today.start} → ${today.stop_or_now || "…"}` : "not started yet";
  $("todayHours").textContent = (today.hours_so_far ?? 0).toFixed(2) + "h";
  $("todayLive").style.display = today.is_running ? "block" : "none";
  $("todayAuto").textContent = today.subtract_auto_minutes == null ? "n/a" : `${today.subtract_auto_minutes} min`;
  const inp = $("todaySubtract");
  if (document.activeElement !== inp) inp.value = today.subtract_manual_minutes ?? 0;
  $("modeBadge").textContent = settings.use_auto_subtract ? "stats: auto" : "stats: manual";
}

function renderSettings() {
  $("csvPath").textContent = settings.csv_path || "—";
  $("useAutoToggle").checked = !!settings.use_auto_subtract;
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

function barData() {
  const m = byDate();
  const todayStr = fmtLocal(new Date());
  if (view === "week") {
    return {
      title: "This week (Mon–Sun)",
      items: weekDays().map((d) => {
        const it = dayItem(d, m, todayStr);
        it.label = d.toLocaleDateString("en-US", { weekday: "short" });
        return it;
      }),
    };
  }
  if (view === "month") {
    return {
      title: "This month (per day)",
      items: monthDays().map((d) => {
        const it = dayItem(d, m, todayStr);
        it.label = String(d.getDate());
        it.sub = "";
        return it;
      }),
    };
  }
  if (view === "custom") {
    const todayStr2 = fmtLocal(new Date());
    let sStr = customPreset === "all" ? earliestEntryDate() || todayStr2 : customStart;
    let s = parseLocalDate(sStr);
    let e = parseLocalDate(customEnd);
    if (!s || !e || s > e) return { title: "Custom", items: [], grouped: false };
    // hard cap so the DOM stays usable (All time with years of data still fine: weekly bars)
    const maxStart = new Date(e);
    maxStart.setDate(maxStart.getDate() - 730);
    if (s < maxStart) s = maxStart;
    const days = Math.round((e - s) / 86400000) + 1;
    if (days > 62) {
      return { title: `Custom (${fmtRangeLabel()}, by week)`, items: groupWeeks(fmtLocal(s), fmtLocal(e), m, todayStr), grouped: true };
    }
    const items = [];
    for (let i = 0; i < days; i++) {
      const d = new Date(s);
      d.setDate(s.getDate() + i);
      const it = dayItem(d, m, todayStr);
      it.label = fmtLocal(d).slice(5);
      it.sub = "";
      items.push(it);
    }
    return { title: `Custom (${fmtRangeLabel()})`, items, grouped: false };
  }
  if (view === "byweek") return { title: "Total hours per week (last 8)", items: lastNWeeks(8) };
  return { title: "Total hours per month (last 6)", items: lastNMonths(6) };
}

function renderAll() {
  const { title, items, grouped } = barData();
  $("viewTitle").textContent = `${title} · ${settings.use_auto_subtract ? "auto subtract" : "manual subtract"}`;
  if (view === "custom") $("rangeLabel").textContent = fmtRangeLabel();
  const vals = items.filter((i) => !i.future && !i.na && i.hours != null).map((i) => i.hours);
  const max = Math.max(1, ...vals);
  const chart = $("chart");
  chart.innerHTML = "";
  for (const it of items) {
    const col = document.createElement("div");
    col.className = "bar-col";
    const v = document.createElement("div");
    v.className = "bar-val" + (it.na ? " na" : "");
    v.textContent = it.future ? "" : it.na ? "n/a" : (it.hours ? it.hours.toFixed(it.hours < 10 ? 2 : 1) : "0");
    const b = document.createElement("div");
    const h = (it.future || it.na || !it.hours) ? 8 : Math.max(10, Math.round((it.hours / max) * 170));
    b.className = "bar" + (!it.hours && !it.na ? " zero" : "") + (it.future ? " future" : "") + (it.na ? " na" : "");
    b.style.height = h + "px";
    b.title = `${it.key || it.label}: ${it.hours == null ? "n/a" : it.hours.toFixed(2) + "h"}`;
    const l = document.createElement("div");
    l.className = "bar-label";
    l.textContent = it.label;
    col.append(v, b, l);
    if (it.sub && view !== "month") {
      const s = document.createElement("div");
      s.className = "bar-sub";
      s.textContent = it.sub;
      col.append(s);
    }
    chart.append(col);
  }
  const dataItems = items.filter((i) => !i.future && !i.na && i.hours != null);
  const total = dataItems.reduce((a, b) => a + b.hours, 0);
  const avg = dataItems.length ? total / dataItems.length : 0;
  $("totalH").textContent = total.toFixed(2) + "h";
  $("avgH").textContent = dataItems.length ? avg.toFixed(2) + "h" : "n/a";
  $("daysN").textContent = String(dataItems.length);
  chart.classList.toggle("scroll", view === "custom" && items.length > 14);

  const tb = $("rows");
  tb.innerHTML = "";
  if ((view === "week" || view === "month" || view === "custom") && !grouped) {
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
  const start = e.start || "—";
  const stop = e.stop || (e.date === today?.date && today?.is_running ? "running…" : "—");
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
  htd.textContent = fmtH(e.hours_worked);
  tr.append(td, tda, htd);
  return tr;
}

document.querySelectorAll(".tab").forEach((b) =>
  b.addEventListener("click", () => {
    document.querySelectorAll(".tab").forEach((x) => x.classList.remove("active"));
    b.classList.add("active");
    view = b.dataset.view;
    $("pickerWrap").classList.toggle("hidden", view !== "custom");
    if (view === "custom") $("rangeLabel").textContent = fmtRangeLabel();
    renderAll();
  })
);

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
  const todayStr = fmtLocal(new Date());
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
$("closeSettings").addEventListener("click", () => $("settingsModal").classList.add("hidden"));

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
load();
