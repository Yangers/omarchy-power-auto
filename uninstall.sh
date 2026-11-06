#!/bin/bash
set -euo pipefail

BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

echo "=========================================="
echo "  Uninstalling Omarchy Power Auto"
echo "=========================================="

echo "• Stopping and disabling systemd service..."
systemctl --user stop omarchy-power-auto.service 2>/dev/null || true
systemctl --user disable omarchy-power-auto.service 2>/dev/null || true

echo "• Removing service unit..."
rm -f "$SYSTEMD_USER_DIR/omarchy-power-auto.service"
systemctl --user daemon-reload

echo "• Removing binary..."
rm -f "$BIN_DIR/omarchy-power-auto"

echo "✅ Uninstalled successfully."
echo "=========================================="
