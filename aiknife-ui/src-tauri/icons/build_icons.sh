#!/usr/bin/env bash

# Get the directory where this script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# Get the path to the input icon
ICON_PATH="$SCRIPT_DIR/icon.png"

# Change to the aiknife-ui directory (two levels up)
cd "$SCRIPT_DIR/../.." || exit 1

# Run pnpm tauri icon with the absolute path to icon.png
pnpm tauri icon "$ICON_PATH"