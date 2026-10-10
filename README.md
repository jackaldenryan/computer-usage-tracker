# Work Hours Tracker (macOS, Tauri)

Automatic daily computer-usage tracker. Uses `pmset -g log` in the background:
first `Display is turned on` = start, last `Display is turned off` = stop,
display-off gaps between = automatic subtract.

- One CSV row per day: `date,start,stop,subtract_manual_minutes,subtract_auto_minutes,hours_worked`
  - Fixed home: `~/Library/Application Support/com.workhours.tracker/work-hours.csv`
    (single location by design, so background tracking never needs permissions).
  - Empty subtract = n/a. Missing row = n/a (no data, excluded from averages).
  - `hours_worked` is final: tracked rows = gross minus the active mode's subtract;
    uploaded rows = trusted as-is in both modes.
- Manual subtract entered per day (defaults 0, saved immediately) + automatic
  gap subtract measured from the log. Settings toggle picks which one all
  statistics (charts, totals, averages) use.
- Backfill upload: Settings → Upload backfill CSV. Required: `date` (YYYY-MM-DD)
  + `hours_worked` (final). Optional: `start`, `stop`,
  `subtract_manual_minutes`, `subtract_auto_minutes`. Missing subtracts = n/a.
- Auto-track only fills days within `pmset` log coverage (~1–2 weeks); older
  gaps stay n/a unless you upload them — or install the background job below,
  which lets unfillable gaps on/after its first run be recorded as 0h.
- Views: This week / This month / Custom range (calendar picker with the same
  presets as cursor-cost-tracking: to-date + rolling + All time) / By week /
  By month, with total + average (n/a and future days excluded).

## Dev

```bash
npm install
npm run tauri dev
```

## Build (Mac)

```bash
npm run tauri build
```

## Background tracking (hourly, no window)

Primary path — entirely inside the app, no Terminal or System Settings:

1. Settings → **Hourly background tracking** → **Enable hourly tracking**.
2. The app copies its bundled `wht-track` helper into Application Support,
   registers the LaunchAgent (runs every hour at :05), test-runs it once, and
   shows the last-run line right in Settings. Disable anytime with the same
   button.

This works with zero permission prompts: the CSV has one fixed home,
`~/Library/Application Support/com.workhours.tracker/work-hours.csv`, which
neither the app nor the helper needs consent for. (An older version let you
relocate it; on upgrade any existing file is moved here automatically.)
macOS never lets a headless process into ~/Documents, ~/Desktop or
~/Downloads without a manual Full Disk Access grant — one more reason the
location is fixed.

- Missed runs (off/asleep) are skipped by launchd, not queued — harmless,
  because every run reconstructs all missing days idempotently.
- First successful run stamps `background_epoch` in settings. From that date
  on, gaps the log can no longer fill are recorded as **0h**: had the machine
  been awake ~1h any of those days, an hourly run would have captured it while
  the log still covered it. Gaps *before* the epoch stay n/a.
- Caveats: the 0h assumption trusts the job is installed and healthy (the
  last-run line is the audit trail); a day of only sub-hour bursts dodging
  every :05 would also read as 0, which for job-hours purposes is ~0 anyway.

Script alternative (same result, for automation):

```bash
./background/install.sh    # builds wht-track, registers the LaunchAgent
./background/uninstall.sh  # removes the LaunchAgent (data kept)
```

- Runs every hour at :05 via `~/Library/LaunchAgents/com.workhours.tracker.track.plist`,
  sharing the GUI's settings + CSV. One line per run in
  `~/Library/Logs/work-hours-tracker/wht-track.log` (audit trail).
- Missed runs (off/asleep) are skipped by launchd, not queued — harmless,
  because every run reconstructs all missing days idempotently.
- First successful run stamps `background_epoch` in settings. From that date
  on, gaps the log can no longer fill are recorded as **0h**: had the machine
  been awake ~1h any of those days, an hourly run would have captured it while
  the log still covered it. Gaps *before* the epoch stay n/a.
- Caveats: the 0h assumption trusts the job is installed and healthy (check
  the run log); a day of only sub-hour bursts dodging every :05 would also read
  as 0, which for job-hours purposes is ~0 anyway.

## Release

Same process as custom-mac-dictation. The version lives in `VERSION`
(must match `package.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`):

```bash
# bump VERSION + the three manifests, commit, then:
./scripts/publish-tag.sh   # checks versions agree + clean tree, tags v$VERSION, pushes
```

Pushing the tag triggers `.github/workflows/release.yml` (macOS runner):
builds the app and attaches `WorkHoursTracker-${version}.dmg` +
`WorkHoursTracker-${version}.zip` to the GitHub Release. Apple Silicon only
for now; Intel Macs are a follow-up. Unsigned build (no Developer ID cert
wired up yet — dictation signs to preserve TCC permissions; we don't need any
TCC permissions by design).
