//! Plays one Masko mascot release. The engine decides which clip plays and
//! when (spec/rules.md); this player keeps the play-time clock, shows the
//! clips the engine names and reports when they show and end
//! (spec/player.md). It is the Windows twin of MaskoPlayer.swift, rule for
//! rule.
//!
//! The player runs on the app's window thread: the app calls `pump` on every
//! turn of its loop (a timer of a few milliseconds is enough), then draws
//! `frame()` when `pump` says it changed, and reads `take_events()`. Files,
//! decoding and the network work on threads of their own.
//!
//! ```ignore
//! let mut player = MaskoPlayer::from_url(
//!     "https://api.masko.ai/v1/canvases/<id>/delivery", "masko_app_...", options);
//! player.set_bool("behavior::pointerHover", true, false);
//! loop {
//!     if player.pump() { draw(player.frame()) }
//!     for event in player.take_events() { /* ... */ }
//! }
//! ```

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use masko_player_engine::{self as engine, ClipKind as EngineKind, Command, Engine};

use crate::clock::{Host, PlayClock, RealHost};
use crate::delivery::{self, EdgeMedia, GraphInfo};
use crate::kept::{Kept, KeptRelease};
use crate::loader::{Done, Loader};
use crate::media::{decode_still, AudioOut, Frame, Pcm};
use crate::slot::{Slot, SlotEvent};

/// The `formats` this player asks the release delivery for (player.md,
/// "Asking for AV1"): it plays stacked AV1 only, decoded with dav1d.
pub const FORMATS: &str = "av1";

/// How long after a failed refresh the player asks again; a first delivery
/// that does not come is asked for again after twice as long each time.
const REFRESH_RETRY: f64 = 30.0;
/// How often a canvas read from its URL asks for a newer release.
const CHECK_EVERY: f64 = 60.0 * 60.0;
/// Ids of the clips' own voices on the sound output; edge sounds come after.
const VOICE_IDS: [u64; 2] = [1, 2];

/// What the app hears (spec/player.md, "What the app sees").
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The mascot is on this pose now.
    Pose { pose: String },
    /// This clip is on screen now: an edge id, or `still:<pose>`.
    Clip {
        clip: String,
        kind: ClipKind,
        rate: f64,
    },
    /// A fire is done; `used` when a start used it (rules.md 3.3).
    ReactionDone { name: String, used: bool },
    /// A move's clip is on screen: move the mascot as the edge's `movement`
    /// says, over `seconds`. The player never moves itself.
    Movement {
        edge: String,
        movement: serde_json::Value,
        rate: f64,
        seconds: f64,
    },
    /// Something went wrong, with a code from the rulebook or the player.
    Error {
        code: String,
        detail: Option<String>,
    },
}

/// What a `Clip` event shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Pose,
    Move,
    Still,
}

/// The clip on screen and how far into it the picture is.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    /// The edge on screen, `still:<pose>`, or None before anything shows.
    pub clip: Option<String>,
    /// Clip time in seconds; 0 for a still or nothing.
    pub time: f64,
}

/// A value the app sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Bool(bool),
    Number(f64),
}

impl From<Value> for engine::Value {
    fn from(v: Value) -> Self {
        match v {
            Value::Bool(b) => engine::Value::Bool(b),
            Value::Number(n) => engine::Value::Number(n),
        }
    }
}

/// How the player is set up.
#[derive(Debug, Clone)]
pub struct Options {
    /// The app's own folder (on Windows `%LOCALAPPDATA%\<app id>`): kept
    /// files go in its `MaskoPlayer` folder.
    pub app_data_dir: PathBuf,
    /// The app's choice, else the graph's `autoPlay`, else true.
    pub auto_play: Option<bool>,
    /// The mascot's size on screen in pixels; pictures are decoded at it.
    pub size: (u32, u32),
    /// The delivery target asked for.
    pub target: String,
    /// Plays sound; false leaves the computer's sound alone (tests).
    pub audio: bool,
}

impl Options {
    pub fn new(app_data_dir: impl Into<PathBuf>, size: (u32, u32)) -> Options {
        Options {
            app_data_dir: app_data_dir.into(),
            auto_play: None,
            size,
            target: delivery::TARGET.to_string(),
            audio: true,
        }
    }
}

/// A call into the engine, kept as data so calls made before the start can
/// be replayed on a scratch engine to find the first frame.
#[derive(Debug, Clone)]
enum Call {
    Set(String, Value, bool),
    Fire(String, bool),
    Reset,
    /// The start: at play time 0, or at now (a release taking over).
    Start {
        at_zero: bool,
    },
    Advance,
    /// Reported only while this showing is still the latest.
    Shown(String, u64),
    Ended(String),
    Failed(String),
    PreviewPose(String),
    PreviewMove(String),
}

fn apply(engine: &mut Engine, call: &Call, now: u64, current: u64) -> Vec<Command> {
    match call {
        Call::Set(name, value, instant) => engine.set(name, (*value).into(), *instant, now),
        Call::Fire(name, instant) => engine.fire(name, *instant, now),
        Call::Reset => engine.reset(now),
        Call::Start { at_zero } => engine.start(if *at_zero { 0 } else { now }),
        Call::Advance => engine.advance(now),
        Call::Shown(edge, token) => {
            if *token == current {
                engine.clip_shown(edge, now)
            } else {
                Vec::new()
            }
        }
        Call::Ended(edge) => engine.clip_ended(edge, now),
        Call::Failed(edge) => engine.clip_failed(edge, now),
        Call::PreviewPose(pose) => engine.preview_show_pose(pose, now),
        Call::PreviewMove(edge) => engine.preview_play_move(edge, now),
    }
}

/// What the player reads from a delivery.
#[derive(Debug, Clone)]
struct Delivery {
    edges: HashMap<String, EdgeMedia>,
    stills: HashMap<String, String>,
    info: GraphInfo,
    expires_at: Option<u64>,
    json: String,
}

impl Delivery {
    fn read(json: &str) -> Result<Delivery, String> {
        Ok(Delivery {
            edges: delivery::edge_media(json).map_err(|e| e.to_string())?,
            stills: delivery::pose_stills(json).map_err(|e| e.to_string())?,
            info: delivery::graph_info(json).map_err(|e| e.to_string())?,
            expires_at: delivery::signed_links_expire_at(json),
            json: json.to_string(),
        })
    }

    /// The file to play for an edge on a mascot `pixels` wide on screen.
    fn file(&self, edge: &str, pixels: Option<f64>) -> Option<(String, String)> {
        self.edges.get(edge)?.file_for_pixels(pixels)
    }

    fn same_release(&self, fresh: &Delivery) -> bool {
        delivery::same_release(&self.json, &fresh.json)
    }

    /// Takes the links of a fresh delivery of the same release.
    fn take_links(&mut self, fresh: &Delivery) {
        for (id, edge) in &fresh.edges {
            if let Some(mine) = self.edges.get_mut(id) {
                mine.video = edge.video.clone();
                mine.variants = edge.variants.clone();
                mine.sound = edge.sound.clone();
            }
        }
        for (id, link) in &fresh.stills {
            self.stills.insert(id.clone(), link.clone());
        }
        self.expires_at = fresh.expires_at;
    }
}

/// A clip or still asked for by the engine (or the first frame shown before
/// the start, a poster).
#[derive(Debug, Clone)]
struct Showing {
    edge: String,
    moving: bool,
    rate: f64,
    token: u64,
    poster: bool,
    ended: bool,
    /// The file chosen for it (`Delivery::file`'s key); none for a still.
    key: Option<String>,
    /// Loaded into its slot (waiting for its first picture or shown).
    loaded: bool,
}

/// What a file asked of the loader is for.
#[derive(Debug, Clone)]
enum Want {
    /// A showing's clip, for this slot.
    Show { slot: usize, token: u64 },
    /// The clip that most likely shows next, decoded ahead in this slot.
    Ahead { slot: usize, key: String },
    /// A preload asked by the engine.
    Preload { edge: String },
    /// A still for this showing.
    Still { token: u64 },
}

/// An edge sound waiting for its samples.
#[derive(Debug, Clone)]
struct PendingSound {
    volume: f32,
    looping: bool,
}

/// Why a release is being kept, and what to do once it is.
enum KeepFor {
    /// The release playing (first delivery or fresh links): nothing more.
    Playing,
    /// A newer release, which takes over once kept.
    Newer,
}

/// A canvas read from its URL with an app key.
struct Source {
    url: String,
    app_key: String,
    install_id: String,
    kept: Arc<Kept>,
    target: String,
}

/// Plays one Masko mascot release.
pub struct MaskoPlayer {
    host: Box<dyn Host>,
    audio: Option<Arc<AudioOut>>,
    loader: Loader,
    events: Vec<Event>,
    /// The picture to draw changed since the last `pump`.
    changed: bool,
    /// Clips' own voices are heard (spec "sound").
    sound: bool,
    size: (u32, u32),

    json: String,
    delivery: Option<Delivery>,
    engine: Option<Engine>,
    refusal: Option<(String, Option<String>)>,

    clock: PlayClock,
    auto_play: bool,
    auto_play_chosen: bool,
    booted: bool,
    first_frame_ready: bool,
    started: bool,
    app_paused: bool,
    host_paused: bool,
    stopped: bool,
    destroyed: bool,

    before_start: Vec<Call>,
    after_start: Vec<Call>,
    held: Vec<Call>,
    queue: VecDeque<Call>,
    draining: bool,
    wake_at: Option<u64>,

    source: Option<Source>,
    checking: Option<Receiver<Result<delivery::Fetched, String>>>,
    keeping: Option<(Receiver<Result<KeptRelease, String>>, KeepFor)>,
    check_at: Option<f64>,
    retry: f64,
    /// A newer release whose files are all kept, waiting for the initial
    /// pose's loop to start again.
    waiting: Option<String>,
    take_over: Option<(String, f64)>,
    /// The values the app set, carried over to a newer release.
    values: BTreeMap<String, Value>,

    slots: [Slot; 2],
    shows: [Option<Showing>; 2],
    visible: Option<usize>,
    still: Option<Showing>,
    still_frame: Option<Frame>,
    token: u64,
    current: u64,
    failed: HashSet<String>,
    loop_ahead: Option<String>,
    wants: HashMap<u64, Want>,
    sounds_pending: HashMap<u64, PendingSound>,
    /// Edge sounds playing, and whether each loops.
    sounds_playing: Vec<(u64, bool)>,
    sound_cache: HashMap<String, Pcm>,
    /// The link each pending sound reads, for the cache.
    sound_links: HashMap<u64, String>,
    next_sound: u64,
}

fn random_seed() -> u64 {
    let mut h = RandomState::new().build_hasher();
    h.write_u64(0x6d61736b6f);
    h.finish().max(1)
}

impl MaskoPlayer {
    /// Plays a canvas's latest release from its URL,
    /// `https://api.masko.ai/v1/canvases/{canvasId}/delivery`, read with an
    /// app key of its mascot (player.md, "A canvas from its URL"). Every file
    /// is downloaded once and kept, so the mascot also plays offline. The
    /// player asks for a newer release at once and every hour, keeps all its
    /// files, then switches to it when the mascot's first pose starts its
    /// loop again.
    pub fn from_url(url: &str, app_key: &str, options: Options) -> MaskoPlayer {
        MaskoPlayer::from_url_with(url, app_key, options, Box::new(RealHost::new()))
    }

    pub(crate) fn from_url_with(
        url: &str,
        app_key: &str,
        options: Options,
        host: Box<dyn Host>,
    ) -> MaskoPlayer {
        let kept = Arc::new(Kept::new(&options.app_data_dir, url));
        let install_id = Kept::install_id(&options.app_data_dir);
        let target = options.target.clone();
        let mut player = MaskoPlayer::blank(&options, host, Some(Arc::clone(&kept)));
        player.source = Some(Source {
            url: url.to_string(),
            app_key: app_key.to_string(),
            install_id,
            kept: Arc::clone(&kept),
            target,
        });
        // Before anything plays: files of older releases go.
        kept.remove_unused();
        if let Some(release) = kept.kept_release() {
            player.load(&release.delivery_json);
        }
        player
    }

    /// Plays a release delivery whose links are local files or links the
    /// player can download into `app_data_dir`.
    pub fn from_delivery(json: &str, options: Options) -> MaskoPlayer {
        MaskoPlayer::from_delivery_with(json, options, Box::new(RealHost::new()))
    }

    pub(crate) fn from_delivery_with(
        json: &str,
        options: Options,
        host: Box<dyn Host>,
    ) -> MaskoPlayer {
        let mut player = MaskoPlayer::blank(&options, host, None);
        player.load(json);
        player
    }

    fn blank(options: &Options, host: Box<dyn Host>, kept: Option<Arc<Kept>>) -> MaskoPlayer {
        let audio = if options.audio {
            AudioOut::new().map(Arc::new)
        } else {
            None
        };
        MaskoPlayer {
            host,
            audio,
            loader: Loader::new(kept),
            events: Vec::new(),
            changed: false,
            sound: true,
            size: options.size,
            json: String::new(),
            delivery: None,
            engine: None,
            refusal: None,
            clock: PlayClock::default(),
            auto_play: options.auto_play.unwrap_or(true),
            auto_play_chosen: options.auto_play.is_some(),
            booted: false,
            first_frame_ready: false,
            started: false,
            app_paused: false,
            host_paused: false,
            stopped: false,
            destroyed: false,
            before_start: Vec::new(),
            after_start: Vec::new(),
            held: Vec::new(),
            queue: VecDeque::new(),
            draining: false,
            wake_at: None,
            source: None,
            checking: None,
            keeping: None,
            check_at: None,
            retry: REFRESH_RETRY,
            waiting: None,
            take_over: None,
            values: BTreeMap::new(),
            slots: [Slot::new(VOICE_IDS[0]), Slot::new(VOICE_IDS[1])],
            shows: [None, None],
            visible: None,
            still: None,
            still_frame: None,
            token: 0,
            current: 0,
            failed: HashSet::new(),
            loop_ahead: None,
            wants: HashMap::new(),
            sounds_pending: HashMap::new(),
            sounds_playing: Vec::new(),
            sound_cache: HashMap::new(),
            sound_links: HashMap::new(),
            next_sound: 100,
        }
    }

    /// Reads a release delivery for the engine and the player. A delivery the
    /// engine refuses plays nothing, with the engine's own reason.
    fn load(&mut self, json: &str) {
        self.json = json.to_string();
        match engine::load(json) {
            Ok(mut e) => match Delivery::read(json) {
                Ok(d) => {
                    e.seed(random_seed());
                    self.engine = Some(e);
                    self.delivery = Some(d);
                    self.refusal = None;
                }
                Err(detail) => {
                    self.engine = None;
                    self.delivery = None;
                    self.refusal = Some(("unreadable".into(), Some(detail)));
                }
            },
            Err(refused) => {
                self.engine = None;
                self.delivery = None;
                self.refusal = Some((refused.code.to_string(), Some(refused.detail)));
            }
        }
        if !self.auto_play_chosen {
            self.auto_play = self
                .delivery
                .as_ref()
                .map(|d| d.info.auto_play)
                .unwrap_or(true);
        }
    }

    // MARK: What the app calls

    /// Holds a boolean until the next `set` of that name (rules.md 3.2).
    /// With `instant`, what it starts happens now (5.8).
    pub fn set_bool(&mut self, name: &str, value: bool, instant: bool) {
        self.set(name, Value::Bool(value), instant);
    }

    /// Holds a number until the next `set` of that name.
    pub fn set_number(&mut self, name: &str, value: f64, instant: bool) {
        self.set(name, Value::Number(value), instant);
    }

    pub fn set(&mut self, name: &str, value: Value, instant: bool) {
        if !self.destroyed {
            self.values.insert(name.to_string(), value);
        }
        self.call(Call::Set(name.to_string(), value, instant), false);
    }

    /// Makes the name read true until a clip uses it (rules.md 3.3).
    pub fn fire(&mut self, name: &str, instant: bool) {
        self.call(Call::Fire(name.to_string(), instant), false);
    }

    /// Values back to their defaults, and the mascot starts again (11.3).
    pub fn reset(&mut self) {
        if self.destroyed || self.engine.is_none() {
            return;
        }
        self.values.clear();
        if !self.started {
            // Nothing has played: forget the values set so far.
            let json = self.json.clone();
            self.load(&json);
            self.before_start.clear();
            if self.booted {
                self.show_poster();
            }
            return;
        }
        self.call(Call::Reset, false);
    }

    /// Cuts to this pose at once (editors and tools, rules.md 13).
    pub fn preview_show_pose(&mut self, pose: &str) {
        self.call(Call::PreviewPose(pose.to_string()), true);
    }

    /// Plays this move from the current pose at once.
    pub fn preview_play_move(&mut self, edge: &str) {
        self.call(Call::PreviewMove(edge.to_string()), true);
    }

    /// Starts the mascot (once its first frame shows), or resumes it exactly
    /// where it paused.
    pub fn play(&mut self) {
        if self.loading() && !self.destroyed {
            self.auto_play = true;
            self.auto_play_chosen = true;
            return;
        }
        if self.destroyed || self.engine.is_none() {
            return;
        }
        if !self.started {
            self.auto_play = true;
            self.auto_play_chosen = true;
            if self.booted && self.first_frame_ready {
                self.start();
            }
            return;
        }
        self.app_paused = false;
        self.apply_pause();
    }

    /// Stops play time, clips, sounds and timers (rules.md 11.2). Before the
    /// start, it keeps the mascot on its first frame until `play()`.
    pub fn pause(&mut self) {
        if self.destroyed {
            return;
        }
        if !self.started {
            self.auto_play = false;
            self.auto_play_chosen = true;
            return;
        }
        self.app_paused = true;
        self.apply_pause();
    }

    /// The mascot can be seen, or not (its window hidden, the screen locked
    /// or off): while not, the player pauses itself.
    pub fn set_host_active(&mut self, active: bool) {
        self.host_paused = !active;
        self.apply_pause();
    }

    /// Lets clips' own voices be heard. On by default; edge sounds play
    /// either way (rules.md 9.6).
    pub fn set_sound(&mut self, on: bool) {
        self.sound = on;
        let uptime = self.host.uptime();
        for i in 0..2 {
            if let Some(s) = &self.shows[i] {
                let muted = !self.voice_on(&s.edge);
                self.slots[i].set_muted(muted, uptime, self.audio.as_deref());
            }
        }
    }

    /// The mascot's size on screen in pixels. A new size picks the files of
    /// the next clips and decodes them at that size.
    pub fn set_size(&mut self, size: (u32, u32)) {
        if size != self.size && size.0 > 0 && size.1 > 0 {
            self.size = size;
            self.prepare_next();
        }
    }

    /// The clip on screen and its clip time.
    pub fn position(&self) -> Position {
        if let Some(v) = self.visible {
            if let Some(s) = &self.shows[v] {
                return Position {
                    clip: Some(s.edge.clone()),
                    time: self.slots[v].clip_time(self.host.uptime()),
                };
            }
        }
        Position {
            clip: self.still.as_ref().map(|s| s.edge.clone()),
            time: 0.0,
        }
    }

    /// The picture to draw: premultiplied BGRA at the size asked for, or
    /// None (draw nothing).
    pub fn frame(&self) -> Option<&Frame> {
        match self.visible {
            Some(v) => self.slots[v].picture(),
            None => self.still_frame.as_ref(),
        }
    }

    /// Every event since the last call, in order.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Takes a fresh delivery of the same release: only its links are used,
    /// the engine keeps running, and clips that failed are tried again.
    /// Another release is refused with `error` `refresh_mismatch`.
    pub fn update_delivery(&mut self, json: &str) -> bool {
        if self.destroyed || self.delivery.is_none() {
            return false;
        }
        let fresh = match Delivery::read(json) {
            Ok(d) => d,
            Err(detail) => {
                self.emit_error("refresh_failed", Some(detail));
                return false;
            }
        };
        if !self.delivery.as_ref().unwrap().same_release(&fresh) {
            self.emit_error(
                "refresh_mismatch",
                Some("the delivery is another release".into()),
            );
            return false;
        }
        self.delivery.as_mut().unwrap().take_links(&fresh);
        let retried: Vec<String> = self.failed.iter().cloned().collect();
        for edge in retried {
            self.preload(&edge);
        }
        true
    }

    /// Stops everything. The player does nothing afterwards; kept files stay.
    pub fn destroy(&mut self) {
        if self.destroyed {
            return;
        }
        self.destroyed = true;
        self.token += 1;
        self.current = self.token;
        self.sounds_pending.clear();
        self.wake_at = None;
        self.check_at = None;
        self.checking = None;
        self.keeping = None;
        self.waiting = None;
        self.stop_sounds(false);
        let audio = self.audio.clone();
        for slot in &mut self.slots {
            slot.unload(audio.as_deref());
        }
        self.engine = None;
    }

    /// Moves the player on: boots it the first time, then takes finished
    /// downloads, decoded pictures and due engine wakes. Returns whether
    /// `frame()` changed.
    pub fn pump(&mut self) -> bool {
        if self.destroyed {
            return false;
        }
        if !self.booted {
            self.boot();
        }
        self.poll_source();
        self.poll_loader();
        self.poll_slots();
        self.poll_wake();
        std::mem::take(&mut self.changed)
    }

    /// When the app should call `pump` again at the latest, in seconds: soon
    /// while a clip plays or a file loads, rarely while nothing moves.
    pub fn next_pump_in(&self) -> f64 {
        if self.destroyed {
            return 1.0;
        }
        if self.visible.is_some() || !self.wants.is_empty() || !self.booted {
            return 1.0 / 120.0;
        }
        0.05
    }

    // MARK: Boot and the start

    fn loading(&self) -> bool {
        self.source.is_some() && self.engine.is_none() && self.refusal.is_none()
    }

    fn boot(&mut self) {
        if self.booted || self.destroyed {
            return;
        }
        self.booted = true;
        if self.source.is_some() {
            self.check();
            // The first delivery plays once it is in.
            if self.engine.is_none() && self.refusal.is_none() {
                return;
            }
        }
        if let Some((code, detail)) = self.refusal.clone() {
            self.emit_error(&code, detail);
            return;
        }
        self.show_poster();
    }

    /// Shows the first frame the start will show, without starting: a
    /// scratch engine replays the calls so far and starts.
    fn show_poster(&mut self) {
        if self.started || self.destroyed {
            return;
        }
        self.first_frame_ready = false;
        let Ok(mut scratch) = engine::load(&self.json) else {
            return self.first_frame_shown();
        };
        for c in self.before_start.clone() {
            apply(&mut scratch, &c, 0, self.current);
        }
        for command in scratch.start(0) {
            match command {
                Command::ShowClip { edge, kind, rate } => {
                    return self.show(&edge, kind == EngineKind::Move, rate, true);
                }
                Command::ShowStill { pose } => return self.show_still(&pose, true),
                _ => {}
            }
        }
        self.first_frame_shown();
    }

    fn first_frame_shown(&mut self) {
        self.first_frame_ready = true;
        if self.auto_play && !self.started {
            self.start();
        }
    }

    /// Play time 0 (rules.md 11.1). The start shows the frame already on
    /// screen and plays it on.
    fn start(&mut self) {
        self.started = true;
        self.app_paused = false;
        let uptime = self.host.uptime();
        self.clock.run(uptime);
        self.send(Call::Start { at_zero: true });
        for c in std::mem::take(&mut self.after_start) {
            self.send(c);
        }
        // Not visible already: the start waits there.
        self.apply_pause();
    }

    fn apply_pause(&mut self) {
        let paused = self.paused();
        if !self.started || self.destroyed || self.stopped == paused {
            return;
        }
        self.stopped = paused;
        let uptime = self.host.uptime();
        let audio = self.audio.clone();
        if paused {
            self.clock.stop(uptime);
            self.wake_at = None;
            if let Some(v) = self.visible {
                self.slots[v].pause(uptime, audio.as_deref());
            }
            if let Some(a) = &audio {
                a.set_paused(true);
            }
            return;
        }
        self.clock.run(uptime);
        // The output runs again first, so a talking clip's voice is heard
        // when the slot says it will be.
        if let Some(a) = &audio {
            a.set_paused(false);
        }
        if let Some(v) = self.visible {
            if let Some(s) = &self.shows[v] {
                if !s.ended {
                    let rate = s.rate;
                    self.slots[v].play(rate, uptime, audio.as_deref());
                }
            }
        }
        for c in std::mem::take(&mut self.held) {
            self.send(c);
        }
        self.schedule_wake();
    }

    fn paused(&self) -> bool {
        self.app_paused || self.host_paused
    }

    // MARK: Calls into the engine

    fn now(&self) -> u64 {
        self.clock.milliseconds(self.host.uptime())
    }

    fn call(&mut self, c: Call, after_start: bool) {
        if self.destroyed || self.engine.is_none() {
            // Values set before a canvas's first delivery choose its first
            // pose (`values` carries them); fires are dropped.
            return;
        }
        if !self.started {
            if after_start {
                self.after_start.push(c);
            } else {
                // Before the start, at play time 0 (rules.md 11.1).
                self.before_start.push(c.clone());
                self.send(c);
                if self.booted {
                    self.show_poster();
                }
            }
            return;
        }
        if self.paused() {
            self.held.push(c);
            return;
        }
        self.send(c);
    }

    /// Calls the engine, runs the commands it returns in order, then
    /// schedules the next wake. A call made while commands run waits its turn.
    fn send(&mut self, c: Call) {
        self.queue.push_back(c);
        if self.draining {
            return;
        }
        self.draining = true;
        while let Some(next) = self.queue.pop_front() {
            let now = self.now();
            let current = self.current;
            let Some(engine) = self.engine.as_mut() else {
                self.queue.clear();
                break;
            };
            for command in apply(engine, &next, now, current) {
                self.execute(command);
            }
            if let Some((edge, rate)) = self.take_over.take() {
                if !self.switch_release() {
                    self.show(&edge, false, rate, false);
                }
            }
        }
        self.draining = false;
        self.prepare_next();
        self.schedule_wake();
    }

    fn schedule_wake(&mut self) {
        self.wake_at = None;
        if !self.started || self.paused() || self.destroyed {
            return;
        }
        self.wake_at = self.engine.as_ref().and_then(|e| e.next_wake());
    }

    fn poll_wake(&mut self) {
        if let Some(at) = self.wake_at {
            if self.now() >= at {
                self.wake_at = None;
                self.send(Call::Advance);
            }
        }
    }

    fn execute(&mut self, command: Command) {
        if self.destroyed {
            return;
        }
        match command {
            Command::ShowClip { edge, kind, rate } => {
                if self.waiting.is_some()
                    && kind == EngineKind::Pose
                    && self.restarts_initial_loop(&edge)
                {
                    // A newer release takes over here, once these commands
                    // are done.
                    self.take_over = Some((edge, rate));
                    return;
                }
                self.show(&edge, kind == EngineKind::Move, rate, false);
            }
            Command::ShowStill { pose } => self.show_still(&pose, false),
            Command::Preload { edge } => self.preload(&edge),
            Command::PlaySound { edge } => self.play_sound(&edge),
            Command::StopSounds { only_looping } => {
                self.sounds_pending
                    .retain(|_, s| only_looping && !s.looping);
                self.stop_sounds(only_looping);
            }
            Command::PoseEntered { pose } => self.events.push(Event::Pose { pose }),
            // The next show replaces the move as soon as it can.
            Command::Interrupted { .. } => {}
            Command::Spent { name, used } => self.events.push(Event::ReactionDone { name, used }),
            Command::Error { code, detail } => {
                if code == "clip_timeout" {
                    // The clip asked for did not show in time and the engine
                    // counts it unplayable (rules.md 9.3): drop it, and let a
                    // later preload of its file mark it playable.
                    for i in 0..2 {
                        if Some(i) != self.visible {
                            if let Some(s) = &self.shows[i] {
                                self.failed.insert(s.edge.clone());
                            }
                        }
                    }
                    self.drop_pending();
                }
                self.events.push(Event::Error { code, detail });
            }
        }
    }

    fn emit_error(&mut self, code: &str, detail: Option<String>) {
        self.events.push(Event::Error {
            code: code.to_string(),
            detail,
        });
    }

    // MARK: Clips

    fn pixels(&self) -> Option<f64> {
        Some(self.size.0.max(self.size.1) as f64)
    }

    fn show(&mut self, edge: &str, moving: bool, rate: f64, poster: bool) {
        self.token += 1;
        let mut showing = Showing {
            edge: edge.to_string(),
            moving,
            rate,
            token: self.token,
            poster,
            ended: false,
            key: None,
            loaded: false,
        };
        self.current = self.token;
        let file = self
            .delivery
            .as_ref()
            .and_then(|d| d.file(edge, self.pixels()));
        if let Some(v) = self.visible {
            if let Some(s) = self.shows[v].clone() {
                let same_file = s.key.as_deref() == file.as_ref().map(|f| f.1.as_str());
                if s.edge == edge && (s.poster || (!s.moving && !moving && same_file)) {
                    // The same pose clip again (or the poster's clip): its
                    // next loop, with no swap.
                    self.drop_pending();
                    if poster {
                        return self.first_frame_shown();
                    }
                    showing.key = s.key.clone();
                    showing.loaded = true;
                    self.shows[v] = Some(showing.clone());
                    let uptime = self.host.uptime();
                    let audio = self.audio.clone();
                    let muted = !self.voice_on(edge);
                    let paused = self.paused();
                    let slot = &mut self.slots[v];
                    slot.set_muted(muted, uptime, audio.as_deref());
                    slot.restart(rate, uptime, audio.as_deref());
                    if paused {
                        slot.pause(uptime, audio.as_deref());
                    }
                    self.announce(&showing);
                    self.report_shown(&showing);
                    return;
                }
            }
        }
        // Otherwise the other slot: load, wait for a decoded frame, swap.
        let slot = if self.visible == Some(0) { 1 } else { 0 };
        showing.key = file.as_ref().map(|f| f.1.clone());
        self.shows[slot] = Some(showing.clone());
        let Some((link, _)) = file else {
            return self.clip_failed(showing, slot);
        };
        let id = self.loader.file(&link);
        self.wants.insert(
            id,
            Want::Show {
                slot,
                token: showing.token,
            },
        );
        self.poll_loader();
    }

    /// The file of a showing is on the computer: load it into its slot.
    fn file_ready(&mut self, slot: usize, token: u64, result: Result<PathBuf, String>) {
        if self.current != token {
            return;
        }
        let Some(showing) = self.shows[slot].clone() else {
            return;
        };
        if showing.token != token {
            return;
        }
        let path = match result {
            Ok(p) => p,
            Err(_) => return self.clip_failed(showing, slot),
        };
        let uptime = self.host.uptime();
        let audio = self.audio.clone();
        let ready = self.slots[slot].load(&path, self.size, uptime, audio.as_deref());
        if let Some(s) = self.shows[slot].as_mut() {
            s.loaded = true;
        }
        if ready {
            self.swap(slot, showing);
        }
    }

    /// Makes the slot visible and hides the other in the same frame.
    fn swap(&mut self, slot: usize, showing: Showing) {
        let uptime = self.host.uptime();
        let audio = self.audio.clone();
        if let Some(old) = self.visible {
            if old != slot {
                self.slots[old].pause(uptime, audio.as_deref());
                self.shows[old] = None;
            }
        }
        self.visible = Some(slot);
        self.still = None;
        self.still_frame = None;
        self.changed = true;
        let muted = !self.voice_on(&showing.edge);
        self.slots[slot].set_muted(muted, uptime, audio.as_deref());
        if showing.poster {
            self.first_frame_shown();
        } else {
            if !self.paused() {
                self.slots[slot].play(showing.rate, uptime, audio.as_deref());
            }
            self.announce(&showing);
            self.report_shown(&showing);
        }
    }

    /// Tells the engine the showing's first frame is on screen (rules 9.3),
    /// once per showing; a showing a later show overtook is not reported.
    fn report_shown(&mut self, showing: &Showing) {
        self.send(Call::Shown(showing.edge.clone(), showing.token));
    }

    fn announce(&mut self, showing: &Showing) {
        self.events.push(Event::Clip {
            clip: showing.edge.clone(),
            kind: if showing.moving {
                ClipKind::Move
            } else {
                ClipKind::Pose
            },
            rate: showing.rate,
        });
        if showing.moving {
            if let Some(edge) = self
                .delivery
                .as_ref()
                .and_then(|d| d.edges.get(&showing.edge))
            {
                if let Some(movement) = edge.movement.clone() {
                    self.events.push(Event::Movement {
                        edge: showing.edge.clone(),
                        movement,
                        rate: showing.rate,
                        seconds: edge.duration_s / showing.rate,
                    });
                }
            }
        }
    }

    /// Forgets a clip loading in the hidden slot.
    fn drop_pending(&mut self) {
        for i in 0..2 {
            if Some(i) != self.visible {
                self.shows[i] = None;
            }
        }
    }

    fn clip_failed(&mut self, showing: Showing, slot: usize) {
        if self.visible != Some(slot)
            && self.shows[slot].as_ref().map(|s| s.token) == Some(showing.token)
        {
            self.shows[slot] = None;
        }
        if showing.poster {
            return self.first_frame_shown();
        }
        self.failed.insert(showing.edge.clone());
        self.send(Call::Failed(showing.edge));
    }

    fn slot_ended(&mut self, slot: usize) {
        if self.visible != Some(slot) {
            return;
        }
        let Some(s) = self.shows[slot].as_mut() else {
            return;
        };
        if s.token != self.current || s.ended || s.poster {
            return;
        }
        s.ended = true;
        let edge = s.edge.clone();
        self.send(Call::Ended(edge));
    }

    fn slot_failed(&mut self, slot: usize) {
        if self.visible != Some(slot) {
            return;
        }
        let Some(s) = self.shows[slot].clone() else {
            return;
        };
        if s.token != self.current || s.ended || s.poster {
            return;
        }
        self.clip_failed(s, slot);
    }

    fn show_still(&mut self, pose: &str, poster: bool) {
        self.token += 1;
        let showing = Showing {
            edge: format!("still:{pose}"),
            moving: false,
            rate: 1.0,
            token: self.token,
            poster,
            ended: false,
            key: None,
            loaded: false,
        };
        self.current = self.token;
        self.drop_pending();
        let link = self
            .delivery
            .as_ref()
            .and_then(|d| d.stills.get(pose).cloned());
        let Some(link) = link else {
            return self.present_still(None, showing);
        };
        let id = self.loader.file(&link);
        self.wants.insert(
            id,
            Want::Still {
                token: showing.token,
            },
        );
        self.still = Some(showing);
        self.poll_loader();
    }

    fn present_still(&mut self, frame: Option<Frame>, showing: Showing) {
        let uptime = self.host.uptime();
        let audio = self.audio.clone();
        if let Some(v) = self.visible {
            self.slots[v].pause(uptime, audio.as_deref());
            self.shows[v] = None;
        }
        self.visible = None;
        self.still_frame = frame;
        self.changed = true;
        let poster = showing.poster;
        let edge = showing.edge.clone();
        self.still = Some(showing);
        if poster {
            self.first_frame_shown();
        } else {
            self.events.push(Event::Clip {
                clip: edge,
                kind: ClipKind::Still,
                rate: 1.0,
            });
        }
    }

    /// Decodes the clip that most likely shows next in the hidden slot, so it
    /// swaps in on time: the decided next clip; during a move, its target
    /// pose's loop; else the pose clip on screen, when the mascot's size now
    /// picks another file for its next loop.
    fn prepare_next(&mut self) {
        if !self.started || self.destroyed {
            return;
        }
        let Some(engine) = self.engine.as_ref() else {
            return;
        };
        let hidden = if self.visible == Some(0) { 1 } else { 0 };
        if self.shows[hidden].is_some() {
            return;
        }
        let on_screen = self.visible.and_then(|v| self.shows[v].clone());
        let mut next = engine.state().next.map(|n| n.clip);
        if next.is_none() {
            if let Some(s) = &on_screen {
                if !s.moving {
                    next = Some(s.edge.clone());
                } else if let (Some(lp), Some(d)) = (&self.loop_ahead, &self.delivery) {
                    let loop_source = d.edges.get(lp).and_then(|e| e.source.clone());
                    let move_target = d.edges.get(&s.edge).and_then(|e| e.target.clone());
                    if loop_source.is_some() && loop_source == move_target {
                        next = Some(lp.clone());
                    }
                }
            }
        }
        let Some(next) = next else { return };
        let Some((link, key)) = self
            .delivery
            .as_ref()
            .and_then(|d| d.file(&next, self.pixels()))
        else {
            return;
        };
        if let Some(s) = &on_screen {
            if s.edge == next && s.key.as_deref() == Some(key.as_str()) {
                return;
            }
        }
        if self
            .wants
            .values()
            .any(|w| matches!(w, Want::Ahead { slot, key: k } if *slot == hidden && *k == key))
        {
            return;
        }
        let id = self.loader.file(&link);
        self.wants.insert(id, Want::Ahead { slot: hidden, key });
    }

    fn preload(&mut self, edge: &str) {
        let is_loop = self
            .delivery
            .as_ref()
            .and_then(|d| d.edges.get(edge))
            .map(|e| e.is_loop)
            .unwrap_or(false);
        if is_loop {
            self.loop_ahead = Some(edge.to_string());
        }
        let Some((link, _)) = self
            .delivery
            .as_ref()
            .and_then(|d| d.file(edge, self.pixels()))
        else {
            return self.mark_unplayable(edge);
        };
        let id = self.loader.file(&link);
        self.wants.insert(
            id,
            Want::Preload {
                edge: edge.to_string(),
            },
        );
    }

    fn mark_unplayable(&mut self, edge: &str) {
        self.failed.insert(edge.to_string());
        if let Some(e) = self.engine.as_mut() {
            e.mark_unplayable(edge);
        }
    }

    // MARK: Sound

    fn voice_on(&self, edge: &str) -> bool {
        self.sound
            && self
                .delivery
                .as_ref()
                .and_then(|d| d.edges.get(edge))
                .map(|e| e.has_audio)
                .unwrap_or(false)
    }

    fn play_sound(&mut self, edge: &str) {
        let Some(sound) = self
            .delivery
            .as_ref()
            .and_then(|d| d.edges.get(edge))
            .and_then(|e| e.sound.clone())
        else {
            return;
        };
        let pending = PendingSound {
            volume: sound.volume,
            looping: sound.looping,
        };
        if let Some(pcm) = self.sound_cache.get(&sound.url).cloned() {
            return self.start_sound(pcm, &pending);
        }
        let id = self.loader.sound(&sound.url);
        self.sounds_pending.insert(id, pending);
        self.sound_links.insert(id, sound.url.clone());
    }

    fn start_sound(&mut self, pcm: Pcm, sound: &PendingSound) {
        let Some(audio) = &self.audio else { return };
        self.next_sound += 1;
        let id = self.next_sound;
        audio.play(id, pcm, sound.volume, sound.looping, false, 0, 1.0);
        self.sounds_playing.push((id, sound.looping));
    }

    fn stop_sounds(&mut self, only_looping: bool) {
        let audio = self.audio.clone();
        self.sounds_playing.retain(|(id, looping)| {
            if only_looping && !looping {
                return true;
            }
            if let Some(a) = &audio {
                a.stop(*id);
            }
            false
        });
    }

    // MARK: Turns

    fn poll_loader(&mut self) {
        for done in self.loader.take() {
            match done {
                Done::File { id, result } => {
                    let Some(want) = self.wants.remove(&id) else {
                        continue;
                    };
                    self.file_done(want, result);
                }
                Done::Sound { id, result } => {
                    let link = self.sound_links.remove(&id);
                    let Some(pending) = self.sounds_pending.remove(&id) else {
                        continue;
                    };
                    if let (Ok(pcm), Some(link)) = (result, link) {
                        self.sound_cache.insert(link, pcm.clone());
                        if !self.destroyed {
                            self.start_sound(pcm, &pending);
                        }
                    }
                }
            }
        }
    }

    fn file_done(&mut self, want: Want, result: Result<PathBuf, String>) {
        if self.destroyed {
            return;
        }
        match want {
            Want::Show { slot, token } => self.file_ready(slot, token, result),
            Want::Ahead { slot, .. } => {
                let Ok(path) = result else { return };
                if self.shows[slot].is_some() || self.visible == Some(slot) {
                    return;
                }
                let uptime = self.host.uptime();
                let audio = self.audio.clone();
                self.slots[slot].load(&path, self.size, uptime, audio.as_deref());
            }
            Want::Preload { edge } => match result {
                Err(_) => self.mark_unplayable(&edge),
                Ok(_) => {
                    if self.failed.remove(&edge) {
                        if let Some(e) = self.engine.as_mut() {
                            e.mark_playable(&edge);
                        }
                    }
                }
            },
            Want::Still { token } => {
                if self.current != token {
                    return;
                }
                let Some(showing) = self.still.clone() else {
                    return;
                };
                let frame = result
                    .ok()
                    .and_then(|p| decode_still(&p, self.size.0, self.size.1).ok());
                self.present_still(frame, showing);
            }
        }
    }

    fn poll_slots(&mut self) {
        let uptime = self.host.uptime();
        for i in 0..2 {
            let (changed, event) = self.slots[i].poll(uptime);
            if changed && self.visible == Some(i) {
                self.changed = true;
            }
            match event {
                Some(SlotEvent::Ready(ok)) => {
                    let Some(showing) = self.shows[i].clone() else {
                        continue;
                    };
                    if showing.token != self.current || !showing.loaded || self.visible == Some(i) {
                        continue;
                    }
                    if ok {
                        self.swap(i, showing);
                    } else {
                        self.clip_failed(showing, i);
                    }
                }
                Some(SlotEvent::Ended) => self.slot_ended(i),
                Some(SlotEvent::Failed) => self.slot_failed(i),
                None => {}
            }
        }
    }

    // MARK: A canvas from its URL

    /// Asks the canvas URL for its latest release (player.md, "A canvas from
    /// its URL"), on a thread of its own.
    fn check(&mut self) {
        self.check_at = None;
        let Some(source) = &self.source else { return };
        let (tx, rx) = channel();
        let (url, key, install, target) = (
            source.url.clone(),
            source.app_key.clone(),
            source.install_id.clone(),
            source.target.clone(),
        );
        let _ = std::thread::Builder::new()
            .name("masko-check".into())
            .spawn(move || {
                let _ = tx.send(
                    delivery::fetch(&url, &key, &install, &target).map_err(|e| e.to_string()),
                );
            });
        self.checking = Some(rx);
    }

    fn check_again(&mut self, delay: f64) {
        self.check_at = Some(self.host.uptime() + delay);
    }

    fn poll_source(&mut self) {
        if self.source.is_none() {
            return;
        }
        if let Some(rx) = &self.checking {
            match rx.try_recv() {
                Ok(result) => {
                    self.checking = None;
                    self.checked(result);
                }
                Err(TryRecvError::Disconnected) => {
                    self.checking = None;
                    self.check_failed("the check stopped".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some((rx, _)) = &self.keeping {
            match rx.try_recv() {
                Ok(result) => {
                    let (_, purpose) = self.keeping.take().unwrap();
                    self.kept(result, purpose);
                }
                Err(TryRecvError::Disconnected) => {
                    self.keeping = None;
                    self.check_failed("keeping the files stopped".into());
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(at) = self.check_at {
            if self.host.uptime() >= at && self.checking.is_none() && self.keeping.is_none() {
                self.check();
            }
        }
    }

    fn checked(&mut self, result: Result<delivery::Fetched, String>) {
        if self.destroyed {
            return;
        }
        let fetched = match result {
            Ok(f) => f,
            Err(detail) => return self.check_failed(detail),
        };
        let fresh = match Delivery::read(&fetched.delivery_json) {
            Ok(d) => d,
            Err(detail) => return self.check_failed(detail),
        };
        if self.engine.is_none() {
            // Nothing plays yet: this one plays now, from its links, while
            // every file is kept.
            self.first(&fetched.delivery_json);
            if self.engine.is_some() {
                return self.keep(fetched, KeepFor::Playing);
            }
        } else if self.delivery.as_ref().map(|d| d.same_release(&fresh)) == Some(true) {
            self.update_delivery(&fetched.delivery_json);
            // Completes the kept files when some are missing.
            return self.keep(fetched, KeepFor::Playing);
        } else if let Err(refused) = engine::load(&fetched.delivery_json) {
            // The release playing goes on.
            self.emit_error(refused.code, Some(refused.detail));
        } else {
            return self.keep(fetched, KeepFor::Newer);
        }
        self.retry = REFRESH_RETRY;
        self.check_again(CHECK_EVERY);
    }

    fn check_failed(&mut self, detail: String) {
        self.emit_error("refresh_failed", Some(detail));
        if self.engine.is_none() {
            let retry = self.retry;
            self.check_again(retry);
            self.retry = (self.retry * 2.0).min(CHECK_EVERY);
        } else {
            self.check_again(CHECK_EVERY);
        }
    }

    fn keep(&mut self, fetched: delivery::Fetched, purpose: KeepFor) {
        let Some(source) = &self.source else { return };
        let kept = Arc::clone(&source.kept);
        let (tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("masko-keep".into())
            .spawn(move || {
                let _ = tx.send(kept.keep(&fetched).map_err(|e| e.to_string()));
            });
        self.keeping = Some((rx, purpose));
    }

    fn kept(&mut self, result: Result<KeptRelease, String>, purpose: KeepFor) {
        if self.destroyed {
            return;
        }
        match result {
            Err(detail) => return self.check_failed(detail),
            Ok(release) => {
                if let KeepFor::Newer = purpose {
                    self.waiting = Some(release.delivery_json);
                    if !self.started {
                        self.switch_release();
                    }
                }
            }
        }
        self.retry = REFRESH_RETRY;
        self.check_again(CHECK_EVERY);
    }

    /// The first delivery of a canvas read from its URL. The values the app
    /// set meanwhile choose the first pose (rules.md 11.1).
    fn first(&mut self, json: &str) {
        self.load(json);
        if let Some((code, detail)) = self.refusal.clone() {
            return self.emit_error(&code, detail);
        }
        let carried: Vec<Call> = self
            .values
            .iter()
            .map(|(name, value)| Call::Set(name.clone(), *value, false))
            .collect();
        for c in carried {
            self.before_start.push(c.clone());
            self.send(c);
        }
        self.show_poster();
    }

    /// Whether this pose clip is the one on screen starting again on the
    /// initial pose: the moment a newer release can take over unseen.
    fn restarts_initial_loop(&self, edge: &str) -> bool {
        let Some(v) = self.visible else { return false };
        let Some(s) = &self.shows[v] else {
            return false;
        };
        if s.edge != edge || s.moving || s.poster {
            return false;
        }
        let Some(d) = &self.delivery else {
            return false;
        };
        let (Some(e), Some(pose)) = (d.edges.get(edge), d.info.initial_node.as_ref()) else {
            return false;
        };
        e.source.as_ref() == Some(pose) && e.target.as_ref() == Some(pose)
    }

    /// Plays the waiting release from now on. Before the start it gives the
    /// first frame; after it, it starts where the initial pose's loop would
    /// have started again, the picture on screen staying until its first
    /// clip swaps in, and the values the app set carry over. A release the
    /// engine refuses is reported and the one playing goes on (false).
    fn switch_release(&mut self) -> bool {
        let Some(json) = self.waiting.take() else {
            return false;
        };
        if self.destroyed {
            return false;
        }
        let before = (self.json.clone(), self.engine.take(), self.delivery.take());
        self.load(&json);
        if self.engine.is_none() || self.refusal.is_some() {
            let refused = self.refusal.take();
            self.json = before.0;
            self.engine = before.1;
            self.delivery = before.2;
            if let Some((code, detail)) = refused {
                self.emit_error(&code, detail);
            }
            return false;
        }
        self.failed.clear();
        self.loop_ahead = None;
        self.sounds_pending.clear();
        if !self.started {
            if let Some(engine) = self.engine.as_mut() {
                for c in &self.before_start {
                    apply(engine, c, 0, self.current);
                }
            }
            if self.booted {
                self.show_poster();
            }
            return true;
        }
        self.stop_sounds(true);
        self.drop_pending();
        if let Some(v) = self.visible {
            self.shows[v] = None;
        }
        self.still = None;
        self.clock = PlayClock::default();
        self.clock.run(self.host.uptime());
        let mut calls: Vec<Call> = vec![Call::Start { at_zero: false }];
        calls.extend(
            self.values
                .iter()
                .map(|(name, value)| Call::Set(name.clone(), *value, false)),
        );
        for c in calls.into_iter().rev() {
            self.queue.push_front(c);
        }
        true
    }
}
