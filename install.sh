#!/bin/bash
set -euo pipefail

# Omarchy Power Auto - Native Rust Installer
# Intelligent Power & Thermal Profile Manager for Omarchy Linux

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"

echo "=========================================="
echo "  Installing Omarchy Power Auto (Rust)"
echo "=========================================="

if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo (Rust toolchain) is required to build from source." >&2
    exit 1
fi

mkdir -p "$BIN_DIR"
mkdir -p "$SYSTEMD_USER_DIR"

echo "• Building release binary with optimizations..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

echo "• Installing binary to $BIN_DIR/omarchy-power-auto..."
install -m 755 "$REPO_DIR/target/release/omarchy-power-auto" "$BIN_DIR/omarchy-power-auto"

echo "• Installing systemd user service..."
cp "$REPO_DIR/systemd/omarchy-power-auto.service" "$SYSTEMD_USER_DIR/omarchy-power-auto.service"

echo "• Reloading and starting service..."
systemctl --user daemon-reload
systemctl --user enable --now omarchy-power-auto.service

echo ""
echo "✅ Installation complete!"
echo ""
echo "Status check:"
"$BIN_DIR/omarchy-power-auto" --status
echo "=========================================="
