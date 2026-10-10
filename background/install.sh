#!/bin/bash
# Install the hourly background tracker (no GUI, no Dock icon).
# Builds the release helper, installs it to Application Support,
# and registers the LaunchAgent (runs every hour at :05).
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$HOME/Library/Application Support/com.workhours.tracker"
BIN_DIR="$APP_DIR/bin"
LOG_DIR="$HOME/Library/Logs/work-hours-tracker"
PLIST_DST="$HOME/Library/LaunchAgents/com.workhours.tracker.track.plist"
LABEL="com.workhours.tracker.track"

echo "Building wht-track (release)..."
cargo build --release --bin wht-track --manifest-path "$REPO_DIR/src-tauri/Cargo.toml"

mkdir -p "$BIN_DIR" "$LOG_DIR"
cp "$REPO_DIR/src-tauri/target/release/wht-track" "$BIN_DIR/wht-track"
chmod +x "$BIN_DIR/wht-track"

sed -e "s#__BIN_PATH__#$BIN_DIR/wht-track#" \
    -e "s#__LOG_PATH__#$LOG_DIR/wht-track.log#" \
    "$REPO_DIR/background/com.workhours.tracker.track.plist.template" > "$PLIST_DST"

UID_NUM="$(id -u)"
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$UID_NUM" "$PLIST_DST"

echo "Installed. Hourly runs at :05 -> $LOG_DIR/wht-track.log"
echo "Test-run now: launchctl kickstart gui/$UID_NUM/$LABEL"
echo "Check schedule: launchctl print gui/$UID_NUM/$LABEL | grep -i -A3 calendar"
