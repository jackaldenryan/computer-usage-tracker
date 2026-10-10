#!/bin/bash
# Remove the hourly background tracker (CSV + settings are left untouched).
set -euo pipefail
LABEL="com.workhours.tracker.track"
UID_NUM="$(id -u)"
launchctl bootout "gui/$UID_NUM/$LABEL" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/com.workhours.tracker.track.plist"
echo "Background job removed. (CSV, settings, and log files kept.)"
