//! Library, likes, playlists (same files as the Python version) and the LRC parser.
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

fn text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Value::deserialize(d)?.as_str().unwrap_or("").to_string())
}

fn num<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Value::deserialize(d)?.as_f64().unwrap_or(0.0))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Track {
    #[serde(default, deserialize_with = "text")]
    pub id: String,
    #[serde(default, deserialize_with = "text")]
    pub title: String,
    #[serde(default, deserialize_with = "text")]
    pub artist: String,
    #[serde(default, deserialize_with = "text")]
    pub src: String,
    #[serde(default, deserialize_with = "text")]
    pub target: String,
    #[serde(default, deserialize_with = "text")]
    pub path: String,
    #[serde(default, deserialize_with = "text")]
    pub url: String,
    #[serde(default, deserialize_with = "num")]
    pub dur: f64,
    /// Everything else the Python version stored (art, why, ...) is kept untouched.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

pub struct Store {
    pub dir: PathBuf,
    pub lib: HashMap<String, Track>,
    pub pls: BTreeMap<String, Vec<String>>,
    pub likes: Vec<String>,
}

impl Store {
    pub fn load() -> Store {
        let dir = dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("melodia");
        let _ = std::fs::create_dir_all(dir.join("lyrics"));
        let read = |n: &str| std::fs::read_to_string(dir.join(n)).ok();
        let lib: HashMap<String, Track> = read("library.json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let pls: BTreeMap<String, Vec<String>> = read("playlists.json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let likes: Vec<String> = read("likes.json").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Store { dir, lib, pls, likes }
    }

    pub fn save(&self) {
        let w = |n: &str, s: Option<String>| {
            if let Some(s) = s {
                let tmp = self.dir.join(format!("{n}.rs.tmp"));
                if std::fs::write(&tmp, s).is_ok() {
                    let _ = std::fs::rename(&tmp, self.dir.join(n));
                }
            }
        };
        w("library.json", serde_json::to_string(&self.lib).ok());
        w("playlists.json", serde_json::to_string(&self.pls).ok());
        w("likes.json", serde_json::to_string(&self.likes).ok());
    }

    pub fn tracks(&self, ids: &[String]) -> Vec<Track> {
        ids.iter().filter_map(|i| self.lib.get(i).cloned()).collect()
    }

    pub fn is_liked(&self, id: &str) -> bool {
        self.likes.iter().any(|l| l == id)
    }

    pub fn all_sorted(&self) -> Vec<Track> {
        let mut v: Vec<Track> = self.lib.values().cloned().collect();
        v.sort_by(|a, b| (a.artist.to_lowercase(), a.title.to_lowercase()).cmp(&(b.artist.to_lowercase(), b.title.to_lowercase())));
        v
    }
}

pub struct Line {
    pub t: f64,
    pub text: String,
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// [mm:ss.xx] lines (also enhanced LRC with <word> tags); several time tags per line are supported.
pub fn parse_lrc(s: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in s.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while rest.starts_with('[') {
            let Some(end) = rest.find(']') else { break };
            let tag = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some((m, sec)) = tag.split_once(':') {
                if let (Ok(m), Ok(sec)) = (m.parse::<f64>(), sec.parse::<f64>()) {
                    times.push(m * 60.0 + sec);
                }
            }
        }
        if times.is_empty() {
            continue;
        }
        let text = strip_tags(rest);
        for t in times {
            out.push(Line { t, text: text.clone() });
        }
    }
    out.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    out
}
