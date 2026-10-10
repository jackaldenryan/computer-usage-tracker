# Work Hours Tracker

Download the Mac app from [Releases](https://github.com/jackaldenryan/work-hours-tracker/releases/latest), drag it into Applications, and open it. It tracks the hours your Mac was in use each day — automatically, from the system power log — and shows them as bar charts with totals and averages.

## Install

1. Open the [latest release](https://github.com/jackaldenryan/work-hours-tracker/releases/latest).
2. Download **WorkHoursTracker-x.y.z.dmg**.
3. Open the disk image and drag **Work Hours Tracker** into Applications.
4. Open **Work Hours Tracker** from Applications.

The app is unsigned. If macOS says it cannot be opened, right-click the app, choose Open, and confirm. You can also run this once:

```
xattr -cr "/Applications/Work Hours Tracker.app"
```

Then open it again from Applications.

A `.zip` is attached to the same release. You do not need it for a first install.

## First launch

No permissions to grant — the app never asks for anything. On first open it:

1. Creates its CSV at `~/Library/Application Support/com.workhours.tracker/work-hours.csv`.
2. Backfills roughly the past 1–2 weeks from the Mac power log (older days stay n/a).
3. Shows this week so far in the bar chart.

Then open Settings and click **Enable hourly tracking** so hours keep recording even when the app is closed.

## What to try after it is installed

- Check the Today card: it shows today's start time and live hours so far.
- Enter minutes to subtract for today; the value saves immediately and the hours update.
- Find a day with a large Auto− value and flip the manual/auto toggle in Settings; its bar should change.
- Switch to By week / By month, then Custom… and pick a range on the calendar.
- Open the CSV in Finder and import it into Google Sheets (File → Import → Upload).
- Quit the app and reopen it: nothing is lost — the power log backfills on launch.

## Updates

The app checks GitHub Releases when it opens and every hour after that. Settings also has **Check for updates**. If a newer version exists, a toast appears bottom-right: **Restart and update**, or **Later**.

## How it works

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

Local builds need the updater signing key (CI gets it from repo secrets):

```bash
export TAURI_SIGNING_PRIVATE_KEY=~/.tauri/work-hours-tracker.key
```

The keypair lives outside the repo (`~/.tauri/`); the public key is baked into
`src-tauri/tauri.conf.json`, the private key in the `TAURI_SIGNING_PRIVATE_KEY`
repo secret. Lose the private key and old installs can never verify a new
update again — back it up.

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
