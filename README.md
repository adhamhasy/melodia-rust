# Melodia (Rust)
Lean version of Melodia: GTK4 + libadwaita + GStreamer. There are no per-frame animation loops (the UI wakes 4 times a second).
Run `./install.sh` once: it installs the packages, builds, and adds "Melodia (Rust)" to your app menu.

It shares data with the Python version (~/.config/melodia): library, likes, playlists, lyric cache and downloads.

Included: YouTube search (studio/audio uploads first), play/pause/seek/volume/next/prev, likes, playlists (create, add),
synced lyrics (LRCLIB, click a line to seek), offline downloads made by the Python version, Spotify-style dark look.
Not yet (later phases): home recommendations, Spotify import, word-by-word and animated lyric looks, full-screen player,
MPRIS/media keys, GNOME extension bridge, Minecraft bridge, AI lyrics.
