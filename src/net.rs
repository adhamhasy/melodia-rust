//! YouTube search/stream links through yt-dlp, and synced lyrics from LRCLIB.
use crate::data::Track;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn ytdlp(args: &[&str]) -> Option<String> {
    let out = Command::new("yt-dlp").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

fn score(title: &str, channel: &str, dur: f64) -> f64 {
    let t = title.to_lowercase();
    let mut s = 0.0;
    if channel.ends_with("- Topic") {
        s += 6.0;
    }
    if t.contains("audio") {
        s += 3.0;
    }
    for bad in ["official video", "music video", "(mv)", "reaction", "karaoke", "8d", "nightcore", "slowed", "sped up", "full album", "compilation", "live"] {
        if t.contains(bad) {
            s -= 8.0;
            break;
        }
    }
    if dur <= 0.0 || dur > 600.0 {
        s -= 10.0;
    } else if dur > 420.0 {
        s -= 2.0;
    }
    s
}

/// Search YouTube and return studio/audio uploads first (video clips and very long uploads are pushed down).
fn search_ranked(query: &str, n: usize) -> Vec<Track> {
    let q = format!("ytsearch{n}:{query}");
    let Some(raw) = ytdlp(&[q.as_str(), "--flat-playlist", "-J"]) else { return vec![] };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else { return vec![] };
    let mut found: Vec<(f64, Track)> = Vec::new();
    for e in v["entries"].as_array().cloned().unwrap_or_default() {
        let Some(id) = e["id"].as_str() else { continue };
        let title = e["title"].as_str().unwrap_or("?").to_string();
        let channel = e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("").to_string();
        let dur = e["duration"].as_f64().unwrap_or(0.0);
        let s = score(&title, &channel, dur);
        found.push((s, Track {
            id: format!("yt:{id}"),
            title,
            artist: channel.replace(" - Topic", ""),
            src: "yt".into(),
            target: format!("https://www.youtube.com/watch?v={id}"),
            dur,
            ..Default::default()
        }));
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let good = found.iter().filter(|f| f.0 > -6.0).count();
    let keep = if good >= 3 { good } else { found.len() };
    found.into_iter().take(keep).map(|f| f.1).collect()
}

pub fn yt_search(q: &str) -> Vec<Track> {
    search_ranked(q, 20).into_iter().take(15).collect()
}

/// A direct audio stream URL for a track (Spotify-imported "ytsearch" tracks are matched to the studio version first).
pub fn stream_url(t: &Track) -> Option<String> {
    let mut target = t.target.clone();
    if target.starts_with("ytsearch") {
        let q = target.splitn(2, ':').nth(1).unwrap_or("").to_string();
        target = search_ranked(&format!("{q} audio"), 8).first()?.target.clone();
    }
    let out = ytdlp(&["-f", "bestaudio", "-g", "--no-playlist", target.as_str()])?;
    out.lines().next().map(|s| s.to_string())
}

fn key(t: &Track) -> String {
    format!("{:x}", md5::compute(format!("{}|{}", t.artist, t.title)))
}

/// A song already downloaded by the Python version (or this one) for offline play.
pub fn offline_file(dir: &Path, id: &str) -> Option<PathBuf> {
    let stem = format!("{:x}.", md5::compute(id));
    for e in std::fs::read_dir(dir.join("downloads")).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(&stem) && !name.ends_with(".part") && !name.ends_with(".ytdl") && !name.ends_with(".temp") {
            return Some(e.path());
        }
    }
    None
}

fn clean(s: &str) -> String {
    let (mut out, mut depth) = (String::new(), 0);
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn lrclib(artist: &str, title: &str, dur: f64) -> Option<String> {
    let mut req = ureq::get("https://lrclib.net/api/search").set("User-Agent", "MelodiaRs/0.1").query("track_name", title);
    if !artist.is_empty() {
        req = req.query("artist_name", artist);
    }
    let v: Value = req.timeout(Duration::from_secs(10)).call().ok()?.into_json().ok()?;
    let mut best: Option<(f64, String)> = None;
    for d in v.as_array()? {
        let Some(s) = d["syncedLyrics"].as_str().filter(|s| !s.is_empty()) else { continue };
        let gap = if dur > 0.0 { (d["duration"].as_f64().unwrap_or(0.0) - dur).abs() } else { 0.0 };
        if best.as_ref().map_or(true, |b| gap < b.0) {
            best = Some((gap, s.to_string()));
        }
    }
    best.map(|b| b.1)
}

/// Synced lyrics, cached on disk (same cache files as the Python version).
pub fn lyrics(dir: &Path, t: &Track) -> Option<String> {
    let path = dir.join("lyrics").join(format!("{}.lrc", key(t)));
    if let Ok(s) = std::fs::read_to_string(&path) {
        return Some(s);
    }
    let (mut artist, mut title) = (clean(&t.artist), clean(&t.title));
    if t.src == "yt" {
        if let Some((a, b)) = title.clone().split_once(" - ") {
            artist = clean(a);
            title = clean(b);
        }
    }
    let found = lrclib(&artist, &title, t.dur).or_else(|| lrclib("", &title, t.dur))?;
    let _ = std::fs::write(&path, &found);
    Some(found)
}
