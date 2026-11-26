#!/usr/bin/env bash
# One command: installs everything needed, builds Melodia (Rust) and adds it to your app menu.
set -e
cd "$(dirname "$0")"
echo "==> Installing system packages (needs sudo)..."
sudo apt install -y build-essential pkg-config curl ca-certificates python3-pip \
  libgtk-4-dev libadwaita-1-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
python3 -m pip install --user --break-system-packages -U yt-dlp
if ! command -v cargo >/dev/null 2>&1; then
  echo "==> Installing Rust (minimal)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
. "$HOME/.cargo/env"
echo "==> Building (the first build takes a few minutes)..."
cargo build --release
install -Dm755 target/release/melodia "$HOME/.local/bin/melodia-rs"
mkdir -p "$HOME/.local/share/applications"
sed "s|%h|$HOME|" org.melodia.PlayerRs.desktop > "$HOME/.local/share/applications/org.melodia.PlayerRs.desktop"
echo "==> Done. Start it from the app menu (Melodia Rust) or run: melodia-rs"
echo "    Your existing library, likes, playlists and downloads are picked up automatically."
