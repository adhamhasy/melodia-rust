//! Melodia (Rust): a lean Spotify-style player. GTK4 + libadwaita + GStreamer, no background animation loops.
mod data;
mod net;

use adw::prelude::*;
use data::{parse_lrc, Line, Store, Track};
use gst::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const APP_ID: &str = "org.melodia.PlayerRs";
const CSS: &str = "
window, .mel-root { background: #000; }
.mel-panel { background: #121212; border-radius: 12px; }
.mel-nav { border-radius: 8px; padding: 8px 12px; }
.mel-nav:hover { background: #2a2a2a; }
.mel-row { padding: 6px 10px; border-radius: 8px; }
.mel-row:hover { background: alpha(white, .09); }
.mel-title { font-size: 26px; font-weight: 800; }
.mel-play { background: #1db954; color: #000; border-radius: 999px; min-width: 40px; min-height: 40px; padding: 0; }
.mel-play:hover { background: #3be477; }
.mel-bar { padding: 8px 16px 10px 16px; }
.liked { color: #1db954; }
.dim { opacity: .5; }
.mel-lyric { font-size: 30px; font-weight: 800; opacity: .3; padding: 8px 0; }
.mel-lyric.cur { opacity: 1; color: #1db954; }
";

enum Msg {
    Stream(String, Option<String>),
    Found(Vec<Track>),
    Lyrics(String, Option<String>),
}

struct Player {
    pipe: gst::Element,
    playing: Cell<bool>,
}

impl Player {
    fn new() -> Player {
        gst::init().expect("GStreamer failed to start");
        let pipe = gst::ElementFactory::make("playbin").build().expect("GStreamer playbin is missing (install gstreamer1.0-plugins-base and -good)");
        pipe.set_property("volume", 0.8f64);
        Player { pipe, playing: Cell::new(false) }
    }

    fn play_uri(&self, uri: &str) {
        let _ = self.pipe.set_state(gst::State::Null);
        self.pipe.set_property("uri", uri);
        let _ = self.pipe.set_state(gst::State::Playing);
        self.playing.set(true);
    }

    fn toggle(&self) {
        if self.playing.get() {
            let _ = self.pipe.set_state(gst::State::Paused);
            self.playing.set(false);
        } else {
            let _ = self.pipe.set_state(gst::State::Playing);
            self.playing.set(true);
        }
    }

    fn stop(&self) {
        let _ = self.pipe.set_state(gst::State::Null);
        self.playing.set(false);
    }

    fn seek(&self, secs: f64) {
        let _ = self.pipe.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, gst::ClockTime::from_seconds_f64(secs.max(0.0)));
    }

    fn pos_dur(&self) -> (f64, f64) {
        let p = self.pipe.query_position::<gst::ClockTime>().map(|c| c.seconds_f64()).unwrap_or(0.0);
        let d = self.pipe.query_duration::<gst::ClockTime>().map(|c| c.seconds_f64()).unwrap_or(0.0);
        (p, d)
    }

    fn volume(&self, v: f64) {
        self.pipe.set_property("volume", v);
    }

    /// (reached the end of the song, error text)
    fn bus_events(&self) -> (bool, Option<String>) {
        let (mut eos, mut err) = (false, None);
        if let Some(bus) = self.pipe.bus() {
            while let Some(m) = bus.pop() {
                match m.view() {
                    gst::MessageView::Eos(_) => eos = true,
                    gst::MessageView::Error(e) => err = Some(e.error().to_string()),
                    _ => {}
                }
            }
        }
        (eos, err)
    }
}

fn fmt(s: f64) -> String {
    let s = s.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

struct App {
    toasts: adw::ToastOverlay,
    win: adw::ApplicationWindow,
    store: RefCell<Store>,
    player: Player,
    tx: async_channel::Sender<Msg>,
    queue: RefCell<Rc<Vec<Track>>>,
    idx: Cell<usize>,
    cur: RefCell<Option<Track>>,
    lines: RefCell<Vec<Line>>,
    labels: RefCell<Vec<gtk::Label>>,
    li: Cell<i32>,
    stack: gtk::Stack,
    side: gtk::Box,
    list_title: gtk::Label,
    list_box: gtk::Box,
    list_tracks: RefCell<Rc<Vec<Track>>>,
    search_box: gtk::Box,
    lyrics_box: gtk::Box,
    lyrics_scroll: gtk::ScrolledWindow,
    title_l: gtk::Label,
    artist_l: gtk::Label,
    heart: gtk::Button,
    play_btn: gtk::Button,
    seek: gtk::Scale,
    time_l: gtk::Label,
}

impl App {
    fn toast(&self, s: &str) {
        self.toasts.add_toast(adw::Toast::new(s));
    }

    fn clear(b: &gtk::Box) {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
    }

    fn heart_class(btn: &gtk::Button, liked: bool) {
        if liked {
            btn.add_css_class("liked");
            btn.remove_css_class("dim");
        } else {
            btn.remove_css_class("liked");
            btn.add_css_class("dim");
        }
    }

    fn set_play_icon(&self) {
        self.play_btn.set_icon_name(if self.player.playing.get() { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" });
    }

    // ---------------------------------------------------------------- likes + playlists
    fn toggle_like(&self, t: &Track) -> bool {
        let liked;
        {
            let mut s = self.store.borrow_mut();
            s.lib.entry(t.id.clone()).or_insert_with(|| t.clone());
            let pos = s.likes.iter().position(|l| l == &t.id);
            liked = match pos {
                Some(p) => {
                    s.likes.remove(p);
                    false
                }
                None => {
                    s.likes.insert(0, t.id.clone());
                    true
                }
            };
            s.save();
        }
        let is_cur = self.cur.borrow().as_ref().map_or(false, |c| c.id == t.id);
        if is_cur {
            Self::heart_class(&self.heart, liked);
        }
        self.toast(if liked { "Added to Liked Songs" } else { "Removed from Liked Songs" });
        liked
    }

    fn add_to(&self, name: &str, t: &Track) {
        {
            let mut s = self.store.borrow_mut();
            s.lib.entry(t.id.clone()).or_insert_with(|| t.clone());
            let ids = s.pls.entry(name.to_string()).or_default();
            if !ids.contains(&t.id) {
                ids.push(t.id.clone());
            }
            s.save();
        }
        self.toast(&format!("Added to {name}"));
    }

    fn add_menu(self: &Rc<Self>, btn: &gtk::Button, t: &Track) {
        let pop = gtk::Popover::new();
        let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
        let names: Vec<String> = self.store.borrow().pls.keys().cloned().collect();
        if names.is_empty() {
            b.append(&gtk::Label::new(Some("Create a playlist first (sidebar)")));
        }
        for n in names {
            let item = gtk::Button::builder().label(n.as_str()).css_classes(["flat"]).build();
            let (app, tr, p) = (self.clone(), t.clone(), pop.clone());
            item.connect_clicked(move |_| {
                app.add_to(&n, &tr);
                p.popdown();
            });
            b.append(&item);
        }
        pop.set_child(Some(&b));
        pop.set_parent(btn);
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
    }

    fn new_playlist_dialog(self: &Rc<Self>) {
        let entry = gtk::Entry::builder().placeholder_text("Playlist name").activates_default(true).build();
        let dlg = adw::AlertDialog::new(Some("New playlist"), None);
        dlg.set_extra_child(Some(&entry));
        dlg.add_response("cancel", "Cancel");
        dlg.add_response("ok", "Create");
        dlg.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dlg.set_default_response(Some("ok"));
        let app = self.clone();
        dlg.connect_response(None, move |_, resp| {
            let name = entry.text().trim().to_string();
            if resp == "ok" && !name.is_empty() {
                {
                    let mut s = app.store.borrow_mut();
                    s.pls.entry(name.clone()).or_default();
                    s.save();
                }
                app.rebuild_side();
                app.show_list(&name, Vec::new());
            }
        });
        dlg.present(Some(&self.win));
    }

    // ---------------------------------------------------------------- lists
    fn row(self: &Rc<Self>, t: &Track, list: &Rc<Vec<Track>>, i: usize) -> gtk::Box {
        let row = gtk::Box::builder().spacing(12).css_classes(["mel-row"]).build();
        let info = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).valign(gtk::Align::Center).build();
        info.append(&gtk::Label::builder().label(t.title.as_str()).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build());
        let artist = if t.artist.is_empty() { "Unknown artist" } else { t.artist.as_str() };
        info.append(&gtk::Label::builder().label(artist).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label", "caption"]).build());
        let heart = gtk::Button::builder().icon_name("emblem-favorite-symbolic").css_classes(["flat", "circular"]).valign(gtk::Align::Center).build();
        Self::heart_class(&heart, self.store.borrow().is_liked(&t.id));
        {
            let (app, tr, h) = (self.clone(), t.clone(), heart.clone());
            heart.connect_clicked(move |_| {
                let liked = app.toggle_like(&tr);
                Self::heart_class(&h, liked);
            });
        }
        let add = gtk::Button::builder().icon_name("list-add-symbolic").css_classes(["flat", "circular"]).valign(gtk::Align::Center).tooltip_text("Add to playlist").build();
        {
            let (app, tr) = (self.clone(), t.clone());
            add.connect_clicked(move |b| app.add_menu(b, &tr));
        }
        row.append(&info);
        row.append(&heart);
        row.append(&add);
        let g = gtk::GestureClick::new();
        {
            let (app, list) = (self.clone(), list.clone());
            g.connect_released(move |_, _, _, _| app.play_at(list.clone(), i));
        }
        row.add_controller(g);
        row
    }

    fn fill(self: &Rc<Self>, target: &gtk::Box, tracks: Rc<Vec<Track>>) {
        Self::clear(target);
        for (i, t) in tracks.iter().enumerate().take(400) {
            let r = self.row(t, &tracks, i);
            target.append(&r);
        }
        if tracks.is_empty() {
            target.append(&gtk::Label::builder().label("Nothing here yet.").xalign(0.0).css_classes(["dim-label"]).build());
        }
    }

    fn show_list(self: &Rc<Self>, title: &str, tracks: Vec<Track>) {
        self.list_title.set_text(title);
        let list = Rc::new(tracks);
        *self.list_tracks.borrow_mut() = list.clone();
        self.fill(&self.list_box, list);
        self.stack.set_visible_child_name("list");
    }

    fn nav_btn(label: &str) -> gtk::Button {
        let l = gtk::Label::builder().label(label).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let b = gtk::Button::builder().css_classes(["flat", "mel-nav"]).build();
        b.set_child(Some(&l));
        b
    }

    fn rebuild_side(self: &Rc<Self>) {
        Self::clear(&self.side);
        let b = Self::nav_btn("Search");
        {
            let app = self.clone();
            b.connect_clicked(move |_| app.stack.set_visible_child_name("search"));
        }
        self.side.append(&b);
        let b = Self::nav_btn("Lyrics");
        {
            let app = self.clone();
            b.connect_clicked(move |_| app.stack.set_visible_child_name("lyrics"));
        }
        self.side.append(&b);
        self.side.append(&gtk::Label::builder().label("YOUR LIBRARY").xalign(0.0).margin_top(14).margin_start(12).css_classes(["caption-heading", "dim-label"]).build());
        let b = Self::nav_btn("Liked Songs");
        {
            let app = self.clone();
            b.connect_clicked(move |_| {
                let v = {
                    let s = app.store.borrow();
                    s.tracks(&s.likes)
                };
                app.show_list("Liked Songs", v);
            });
        }
        self.side.append(&b);
        let b = Self::nav_btn("All songs");
        {
            let app = self.clone();
            b.connect_clicked(move |_| {
                let v = app.store.borrow().all_sorted();
                app.show_list("All songs", v);
            });
        }
        self.side.append(&b);
        let names: Vec<String> = self.store.borrow().pls.keys().cloned().collect();
        for n in names {
            let b = Self::nav_btn(n.as_str());
            let app = self.clone();
            b.connect_clicked(move |_| {
                let v = {
                    let s = app.store.borrow();
                    s.tracks(s.pls.get(&n).map(|v| v.as_slice()).unwrap_or(&[]))
                };
                app.show_list(&n, v);
            });
            self.side.append(&b);
        }
        let b = Self::nav_btn("+  New playlist");
        {
            let app = self.clone();
            b.connect_clicked(move |_| app.new_playlist_dialog());
        }
        self.side.append(&b);
    }

    // ---------------------------------------------------------------- playback
    fn play_at(self: &Rc<Self>, list: Rc<Vec<Track>>, i: usize) {
        if i >= list.len() {
            return;
        }
        let t = list[i].clone();
        *self.queue.borrow_mut() = list;
        self.idx.set(i);
        self.start(t);
    }

    fn start(self: &Rc<Self>, t: Track) {
        self.title_l.set_text(&t.title);
        self.artist_l.set_text(&t.artist);
        Self::heart_class(&self.heart, self.store.borrow().is_liked(&t.id));
        *self.cur.borrow_mut() = Some(t.clone());
        self.lines.borrow_mut().clear();
        self.li.set(-1);
        self.lyrics_message("Loading lyrics…");
        let dir = self.store.borrow().dir.clone();
        if t.src == "file" {
            self.player.play_uri(&gtk::gio::File::for_path(&t.path).uri());
        } else if let Some(p) = net::offline_file(&dir, &t.id) {
            self.player.play_uri(&gtk::gio::File::for_path(&p).uri());
        } else if t.src == "stream" && !t.url.is_empty() {
            self.player.play_uri(&t.url);
        } else {
            let (tx, tr) = (self.tx.clone(), t.clone());
            std::thread::spawn(move || {
                let url = net::stream_url(&tr);
                let _ = tx.send_blocking(Msg::Stream(tr.id.clone(), url));
            });
        }
        self.set_play_icon();
        let (tx, tr) = (self.tx.clone(), t);
        std::thread::spawn(move || {
            let txt = net::lyrics(&dir, &tr);
            let _ = tx.send_blocking(Msg::Lyrics(tr.id.clone(), txt));
        });
    }

    fn next(self: &Rc<Self>) {
        let q = self.queue.borrow().clone();
        if q.is_empty() {
            return;
        }
        let i = (self.idx.get() + 1) % q.len();
        self.idx.set(i);
        self.start(q[i].clone());
    }

    fn prev(self: &Rc<Self>) {
        let q = self.queue.borrow().clone();
        if q.is_empty() {
            return;
        }
        if self.player.pos_dur().0 > 3.0 {
            self.player.seek(0.0);
            return;
        }
        let i = (self.idx.get() + q.len() - 1) % q.len();
        self.idx.set(i);
        self.start(q[i].clone());
    }

    // ---------------------------------------------------------------- lyrics
    fn lyrics_message(&self, msg: &str) {
        Self::clear(&self.lyrics_box);
        self.labels.borrow_mut().clear();
        self.lyrics_box.append(&gtk::Label::builder().label(msg).wrap(true).css_classes(["title-3", "dim-label"]).build());
    }

    fn set_lyrics(self: &Rc<Self>, lines: Vec<Line>) {
        if lines.is_empty() {
            self.lyrics_message("No synced lyrics found for this song.");
            return;
        }
        Self::clear(&self.lyrics_box);
        let mut labels = Vec::new();
        for l in &lines {
            let text = if l.text.is_empty() { "♪" } else { l.text.as_str() };
            let lbl = gtk::Label::builder().label(text).xalign(0.0).wrap(true).css_classes(["mel-lyric"]).build();
            let g = gtk::GestureClick::new();
            let (app, t) = (self.clone(), l.t);
            g.connect_released(move |_, _, _, _| app.player.seek(t));
            lbl.add_controller(g);
            self.lyrics_box.append(&lbl);
            labels.push(lbl);
        }
        *self.labels.borrow_mut() = labels;
        *self.lines.borrow_mut() = lines;
        self.li.set(-1);
    }

    fn sync_lyrics(&self, p: f64) {
        let idx = {
            let lines = self.lines.borrow();
            if lines.is_empty() {
                return;
            }
            let mut idx = -1;
            for (i, l) in lines.iter().enumerate() {
                if l.t <= p + 0.05 {
                    idx = i as i32;
                } else {
                    break;
                }
            }
            idx
        };
        if idx == self.li.get() {
            return;
        }
        let labels = self.labels.borrow();
        if self.li.get() >= 0 {
            if let Some(old) = labels.get(self.li.get() as usize) {
                old.remove_css_class("cur");
            }
        }
        self.li.set(idx);
        if idx >= 0 {
            if let Some(lbl) = labels.get(idx as usize) {
                lbl.add_css_class("cur");
                if let Some(pt) = lbl.compute_point(&self.lyrics_box, &gtk::graphene::Point::zero()) {
                    let adj = self.lyrics_scroll.vadjustment();
                    let target = pt.y() as f64 - adj.page_size() / 2.0 + lbl.height() as f64 / 2.0;
                    adj.set_value(target.max(0.0));
                }
            }
        }
    }

    // ---------------------------------------------------------------- 4 times a second, only
    fn tick(self: &Rc<Self>) {
        let (eos, err) = self.player.bus_events();
        if let Some(e) = err {
            self.player.stop();
            self.set_play_icon();
            self.toast(&format!("Playback error: {e}"));
        }
        if eos {
            if self.queue.borrow().len() > 1 {
                self.next();
            } else {
                self.player.stop();
                self.set_play_icon();
            }
            return;
        }
        if !self.player.playing.get() {
            return;
        }
        let (p, d) = self.player.pos_dur();
        if d > 0.0 {
            self.seek.set_range(0.0, d);
            self.seek.set_value(p);
        }
        self.time_l.set_text(&format!("{} / {}", fmt(p), fmt(d)));
        self.sync_lyrics(p);
    }

    fn handle(self: &Rc<Self>, m: Msg) {
        match m {
            Msg::Stream(id, url) => {
                let is_cur = self.cur.borrow().as_ref().map_or(false, |c| c.id == id);
                if is_cur {
                    match url {
                        Some(u) => {
                            self.player.play_uri(&u);
                            self.set_play_icon();
                        }
                        None => self.toast("Couldn't get that song. Is yt-dlp installed and up to date?"),
                    }
                }
            }
            Msg::Found(v) => {
                self.fill(&self.search_box, Rc::new(v));
            }
            Msg::Lyrics(id, txt) => {
                let is_cur = self.cur.borrow().as_ref().map_or(false, |c| c.id == id);
                if is_cur {
                    match txt {
                        Some(t) => self.set_lyrics(parse_lrc(&t)),
                        None => self.lyrics_message("No synced lyrics found for this song."),
                    }
                }
            }
        }
    }
}

fn build_ui(application: &adw::Application) {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
    let (tx, rx) = async_channel::unbounded::<Msg>();

    let win = adw::ApplicationWindow::builder().application(application).title("Melodia").default_width(1100).default_height(720).build();
    let toasts = adw::ToastOverlay::new();
    let root = gtk::Box::builder().orientation(gtk::Orientation::Vertical).css_classes(["mel-root"]).build();
    root.append(&adw::HeaderBar::builder().css_classes(["flat"]).build());
    let body = gtk::Box::builder().spacing(8).vexpand(true).margin_start(8).margin_end(8).build();

    let side = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).margin_top(8).margin_bottom(8).margin_start(6).margin_end(6).build();
    let side_panel = gtk::Box::builder().css_classes(["mel-panel"]).width_request(230).build();
    side_panel.append(&gtk::ScrolledWindow::builder().child(&side).hexpand(true).vexpand(true).build());

    let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
    let search_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_top(16).margin_start(20).margin_end(20).build();
    let entry = gtk::SearchEntry::builder().placeholder_text("What do you want to play? (press Enter)").build();
    let search_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    search_page.append(&entry);
    search_page.append(&gtk::ScrolledWindow::builder().child(&search_box).vexpand(true).build());
    stack.add_named(&search_page, Some("search"));
    let list_page = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(10).margin_top(20).margin_start(20).margin_end(20).build();
    let head = gtk::Box::builder().spacing(14).build();
    let list_title = gtk::Label::builder().xalign(0.0).hexpand(true).css_classes(["mel-title"]).build();
    let list_play = gtk::Button::builder().icon_name("media-playback-start-symbolic").css_classes(["mel-play"]).tooltip_text("Play all").build();
    head.append(&list_title);
    head.append(&list_play);
    let list_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
    list_page.append(&head);
    list_page.append(&gtk::ScrolledWindow::builder().child(&list_box).vexpand(true).build());
    stack.add_named(&list_page, Some("list"));
    let lyrics_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).margin_top(160).margin_bottom(260).margin_start(40).margin_end(40).build();
    let lyrics_scroll = gtk::ScrolledWindow::builder().child(&lyrics_box).vexpand(true).build();
    stack.add_named(&lyrics_scroll, Some("lyrics"));

    let main = gtk::Box::builder().css_classes(["mel-panel"]).hexpand(true).build();
    main.set_overflow(gtk::Overflow::Hidden);
    main.append(&stack);
    body.append(&side_panel);
    body.append(&main);
    root.append(&body);

    let bar = gtk::CenterBox::builder().css_classes(["mel-bar"]).build();
    let title_l = gtk::Label::builder().label("Nothing playing").xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build();
    let artist_l = gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["dim-label"]).build();
    let info = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).width_request(240).build();
    info.append(&title_l);
    info.append(&artist_l);
    let heart = gtk::Button::builder().icon_name("emblem-favorite-symbolic").css_classes(["flat", "circular", "dim"]).valign(gtk::Align::Center).build();
    let left = gtk::Box::builder().spacing(8).width_request(320).build();
    left.append(&info);
    left.append(&heart);
    let prev_b = gtk::Button::builder().icon_name("media-skip-backward-symbolic").css_classes(["flat", "circular"]).build();
    let play_btn = gtk::Button::builder().icon_name("media-playback-start-symbolic").css_classes(["mel-play"]).build();
    let next_b = gtk::Button::builder().icon_name("media-skip-forward-symbolic").css_classes(["flat", "circular"]).build();
    let ctl = gtk::Box::builder().spacing(10).halign(gtk::Align::Center).build();
    ctl.append(&prev_b);
    ctl.append(&play_btn);
    ctl.append(&next_b);
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    seek.set_hexpand(true);
    seek.set_draw_value(false);
    let time_l = gtk::Label::builder().label("0:00 / 0:00").css_classes(["caption"]).build();
    let srow = gtk::Box::builder().spacing(8).build();
    srow.append(&seek);
    srow.append(&time_l);
    let center = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(520).build();
    center.append(&ctl);
    center.append(&srow);
    let vol = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    vol.set_value(0.8);
    vol.set_draw_value(false);
    vol.set_width_request(110);
    let right = gtk::Box::builder().halign(gtk::Align::End).valign(gtk::Align::Center).width_request(320).build();
    right.append(&vol);
    bar.set_start_widget(Some(&left));
    bar.set_center_widget(Some(&center));
    bar.set_end_widget(Some(&right));
    root.append(&bar);
    toasts.set_child(Some(&root));
    win.set_content(Some(&toasts));

    let store = Store::load();
    let start_liked = !store.likes.is_empty();
    let app = Rc::new(App {
        toasts,
        win: win.clone(),
        store: RefCell::new(store),
        player: Player::new(),
        tx,
        queue: RefCell::new(Rc::new(Vec::new())),
        idx: Cell::new(0),
        cur: RefCell::new(None),
        lines: RefCell::new(Vec::new()),
        labels: RefCell::new(Vec::new()),
        li: Cell::new(-1),
        stack,
        side,
        list_title,
        list_box,
        list_tracks: RefCell::new(Rc::new(Vec::new())),
        search_box,
        lyrics_box,
        lyrics_scroll,
        title_l,
        artist_l,
        heart: heart.clone(),
        play_btn: play_btn.clone(),
        seek: seek.clone(),
        time_l,
    });

    {
        let a = app.clone();
        play_btn.connect_clicked(move |_| {
            if a.cur.borrow().is_some() {
                a.player.toggle();
                a.set_play_icon();
            }
        });
        let a = app.clone();
        next_b.connect_clicked(move |_| a.next());
        let a = app.clone();
        prev_b.connect_clicked(move |_| a.prev());
        let a = app.clone();
        heart.connect_clicked(move |_| {
            let t = a.cur.borrow().clone();
            if let Some(t) = t {
                a.toggle_like(&t);
            }
        });
        let a = app.clone();
        seek.connect_change_value(move |_, _, v| {
            a.player.seek(v);
            glib::Propagation::Proceed
        });
        let a = app.clone();
        vol.connect_value_changed(move |s| a.player.volume(s.value()));
        let a = app.clone();
        list_play.connect_clicked(move |_| {
            let l = a.list_tracks.borrow().clone();
            a.play_at(l, 0);
        });
        let a = app.clone();
        entry.connect_activate(move |e| {
            let q = e.text().trim().to_string();
            if q.is_empty() {
                return;
            }
            a.toast("Searching…");
            let tx = a.tx.clone();
            std::thread::spawn(move || {
                let r = net::yt_search(&q);
                let _ = tx.send_blocking(Msg::Found(r));
            });
        });
        let a = app.clone();
        let g = gtk::GestureClick::new();
        g.connect_released(move |_, _, _, _| a.stack.set_visible_child_name("lyrics"));
        info.add_controller(g);
        let a = app.clone();
        glib::spawn_future_local(async move {
            while let Ok(m) = rx.recv().await {
                a.handle(m);
            }
        });
        let a = app.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            a.tick();
            glib::ControlFlow::Continue
        });
    }

    app.rebuild_side();
    if start_liked {
        let v = {
            let s = app.store.borrow();
            s.tracks(&s.likes)
        };
        app.show_list("Liked Songs", v);
    } else {
        app.stack.set_visible_child_name("search");
    }
    win.present();
}

fn main() -> glib::ExitCode {
    if let Some(h) = dirs::home_dir() {
        let p = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{}", h.join(".local/bin").display(), p));
    }
    let application = adw::Application::builder().application_id(APP_ID).build();
    application.connect_activate(build_ui);
    application.run()
}
