//! One of the two video slots (spec/player.md, `showClip`): a stacked AV1
//! clip read, decoded and composed a few pictures ahead on its own thread,
//! and shown at its clip time. The clip's own sound plays from the same file,
//! and the picture waits for it: the clock starts when the sound reaches the
//! speakers, so a voice stays on the lips (as AV1Slot.swift does on the Mac).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::media::{clip_audio, AudioOut, ClipFile, ClipInfo, Frame, Pcm, VideoDecoder};

/// How many pictures the decoder keeps ready ahead of the one on screen.
const LOOKAHEAD: usize = 3;
/// A file that never shows a picture counts as failed after this long.
const LOAD_TIMEOUT: f64 = 10.0;

/// What a slot reports to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotEvent {
    /// The loaded file's first picture is decoded (or the file cannot play).
    Ready(bool),
    /// The clip played its last picture.
    Ended,
    /// The clip stopped playing on an error.
    Failed,
}

/// What the decoding thread and the slot share.
struct Shared {
    /// Bumped by every rewind, so pictures of an older pass are dropped.
    generation: u64,
    /// Decoded pictures in order, with their time in ms from the clip start.
    pictures: VecDeque<(u64, u64, Frame)>,
    /// Pictures the slot is done with, reused by the decoder.
    spare: Vec<Frame>,
    /// The clip, once read.
    info: Option<ClipInfo>,
    voice: Option<Pcm>,
    /// The decoder gave its last picture for this generation.
    done: bool,
    failed: bool,
    rewind: bool,
    stop: bool,
}

struct Worker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(path: PathBuf, width: u32, height: u32) -> Worker {
        let shared = Arc::new((
            Mutex::new(Shared {
                generation: 0,
                pictures: VecDeque::new(),
                spare: Vec::new(),
                info: None,
                voice: None,
                done: false,
                failed: false,
                rewind: false,
                stop: false,
            }),
            Condvar::new(),
        ));
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("masko-decode".into())
            .spawn(move || decode(theirs, &path, width, height))
            .ok();
        if thread.is_none() {
            shared.0.lock().unwrap().failed = true;
        }
        Worker { shared, thread }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Shared) -> R) -> R {
        let (lock, wake) = &*self.shared;
        let mut s = lock.lock().unwrap();
        let r = f(&mut s);
        wake.notify_all();
        r
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.with(|s| s.stop = true);
        // The thread ends after the picture it is decoding; nobody waits.
        self.thread.take();
    }
}

/// The decoding thread: reads the file, then keeps LOOKAHEAD pictures ready.
fn decode(shared: Arc<(Mutex<Shared>, Condvar)>, path: &Path, width: u32, height: u32) {
    let (lock, wake) = &*shared;
    let opened = ClipFile::open(path).and_then(|clip| {
        let decoder = VideoDecoder::new(&clip, width, height)?;
        Ok((clip, decoder))
    });
    let (clip, mut decoder) = match opened {
        Ok(v) => v,
        Err(_) => {
            lock.lock().unwrap().failed = true;
            wake.notify_all();
            return;
        }
    };
    // A voice that cannot be read leaves the clip silent, as on the Mac.
    let voice = clip_audio(&clip).ok().flatten();
    {
        let mut s = lock.lock().unwrap();
        s.info = Some(clip.info().clone());
        s.voice = voice;
        wake.notify_all();
    }
    loop {
        let (generation, mut frame) = {
            let mut s = lock.lock().unwrap();
            while !s.stop && !s.rewind && (s.done || s.pictures.len() >= LOOKAHEAD) {
                s = wake.wait(s).unwrap();
            }
            if s.stop {
                return;
            }
            if s.rewind {
                s.rewind = false;
                s.done = false;
                drop(s);
                if decoder.rewind().is_err() {
                    lock.lock().unwrap().failed = true;
                    wake.notify_all();
                    return;
                }
                continue;
            }
            let frame = s.spare.pop().unwrap_or_else(|| Frame::new(width, height));
            (s.generation, frame)
        };
        let result = decoder.next(&mut frame);
        let mut s = lock.lock().unwrap();
        match result {
            Ok(Some(ms)) if s.generation == generation && !s.rewind => {
                s.pictures.push_back((generation, ms, frame));
            }
            Ok(Some(_)) => s.spare.push(frame),
            Ok(None) => {
                if s.generation == generation && !s.rewind {
                    s.done = true;
                }
                s.spare.push(frame);
            }
            Err(_) => {
                s.failed = true;
                wake.notify_all();
                return;
            }
        }
        wake.notify_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    Idle,
    Loading,
    Ready,
    Failed,
}

/// One slot. The player calls it from one thread and polls it every turn.
pub(crate) struct Slot {
    path: Option<PathBuf>,
    size: (u32, u32),
    worker: Option<Worker>,
    load: Load,
    loading_since: f64,
    info: Option<ClipInfo>,
    voice: Option<Pcm>,
    /// The picture on screen in this slot.
    picture: Option<Frame>,
    generation: u64,
    played: bool,
    /// Clip time in ms when the clock last started or stopped.
    base_ms: f64,
    /// Uptime when the clock started, while it runs; it may lie ahead, until
    /// the clip's voice is heard.
    started_at: Option<f64>,
    rate: f64,
    muted: bool,
    sound_id: u64,
    ended: bool,
}

impl Slot {
    pub fn new(sound_id: u64) -> Slot {
        Slot {
            path: None,
            size: (0, 0),
            worker: None,
            load: Load::Idle,
            loading_since: 0.0,
            info: None,
            voice: None,
            picture: None,
            generation: 0,
            played: false,
            base_ms: 0.0,
            started_at: None,
            rate: 1.0,
            muted: true,
            sound_id,
            ended: false,
        }
    }

    /// Loads a file paused on its first picture; `poll` reports `Ready` once
    /// that picture is decoded. Loading again a file loaded ahead and not
    /// played yet is ready at once. Returns true when it is ready now.
    pub fn load(
        &mut self,
        path: &Path,
        size: (u32, u32),
        uptime: f64,
        audio: Option<&AudioOut>,
    ) -> bool {
        self.pause(uptime, audio);
        if self.path.as_deref() == Some(path) && self.size == size && !self.played {
            match self.load {
                Load::Ready => return true,
                Load::Loading => return false,
                Load::Idle | Load::Failed => {}
            }
        }
        self.unload(audio);
        self.path = Some(path.to_path_buf());
        self.size = size;
        self.load = Load::Loading;
        self.loading_since = uptime;
        self.worker = Some(Worker::start(path.to_path_buf(), size.0, size.1));
        false
    }

    /// Lets go of the file.
    pub fn unload(&mut self, audio: Option<&AudioOut>) {
        self.stop_clock();
        if let Some(a) = audio {
            a.stop(self.sound_id);
        }
        self.worker = None;
        // A new decoding thread counts its passes from 0 again.
        self.generation = 0;
        self.path = None;
        self.load = Load::Idle;
        self.info = None;
        self.voice = None;
        self.picture = None;
        self.played = false;
        self.base_ms = 0.0;
        self.ended = false;
    }

    /// The picture on screen in this slot.
    pub fn picture(&self) -> Option<&Frame> {
        self.picture.as_ref()
    }

    /// Seconds into the clip.
    pub fn clip_time(&self, uptime: f64) -> f64 {
        self.clip_ms(uptime) / 1000.0
    }

    fn clip_ms(&self, uptime: f64) -> f64 {
        let Some(at) = self.started_at else {
            return self.base_ms;
        };
        // Before the voice is heard, the clock holds the picture on screen.
        let t = self.base_ms + (uptime - at).max(0.0) * 1000.0 * self.rate;
        match &self.info {
            Some(info) => t.min(info.duration_ms as f64),
            None => t,
        }
    }

    /// Plays from the picture on screen at this rate.
    pub fn play(&mut self, rate: f64, uptime: f64, audio: Option<&AudioOut>) {
        if self.load != Load::Ready {
            return;
        }
        self.played = true;
        self.base_ms = self.clip_ms(uptime);
        self.rate = rate;
        self.started_at = Some(uptime);
        // The picture starts when the voice is heard, not when it is asked
        // for, so the voice is never behind the lips.
        if !self.muted {
            if let (Some(a), Some(voice)) = (audio, self.voice.clone()) {
                let heard = a.play(
                    self.sound_id,
                    voice,
                    1.0,
                    false,
                    true,
                    self.base_ms as u64,
                    rate as f32,
                );
                self.started_at = Some(uptime + heard as f64 / 1000.0);
            }
        }
    }

    /// Back to the first picture and plays at this rate: the next loop.
    pub fn restart(&mut self, rate: f64, uptime: f64, audio: Option<&AudioOut>) {
        if self.load != Load::Ready {
            return;
        }
        self.stop_clock();
        if let Some(a) = audio {
            a.stop(self.sound_id);
        }
        self.base_ms = 0.0;
        self.ended = false;
        if let Some(w) = &self.worker {
            let generation = w.with(|s| {
                s.generation += 1;
                s.rewind = true;
                s.done = false;
                let old: Vec<Frame> = s.pictures.drain(..).map(|(_, _, f)| f).collect();
                s.spare.extend(old);
                s.generation
            });
            self.generation = generation;
        }
        self.play(rate, uptime, audio);
    }

    pub fn pause(&mut self, uptime: f64, audio: Option<&AudioOut>) {
        if self.started_at.is_none() {
            return;
        }
        self.base_ms = self.clip_ms(uptime);
        self.stop_clock();
        if let Some(a) = audio {
            a.stop(self.sound_id);
        }
    }

    pub fn set_muted(&mut self, muted: bool, uptime: f64, audio: Option<&AudioOut>) {
        if self.muted == muted {
            return;
        }
        self.muted = muted;
        let Some(a) = audio else { return };
        if muted {
            a.stop(self.sound_id);
        } else if self.started_at.is_some() {
            if let Some(voice) = self.voice.clone() {
                // The clock keeps running: the voice joins where the picture
                // will be once it is heard.
                let at = self.clip_ms(uptime) + a.latency_ms() as f64 * self.rate;
                a.play(
                    self.sound_id,
                    voice,
                    1.0,
                    false,
                    true,
                    at as u64,
                    self.rate as f32,
                );
            }
        }
    }

    fn stop_clock(&mut self) {
        self.started_at = None;
    }

    /// Moves the slot on: the first picture of a load, then the picture for
    /// the clock's time, and the end at the clip's length. Returns whether
    /// the picture on screen changed, and what happened.
    pub fn poll(&mut self, uptime: f64) -> (bool, Option<SlotEvent>) {
        let Some(worker) = &self.worker else {
            return (false, None);
        };
        let (lock, wake) = &*worker.shared;
        let mut s = lock.lock().unwrap();
        if s.failed {
            drop(s);
            return match self.load {
                Load::Loading => {
                    self.load = Load::Failed;
                    (false, Some(SlotEvent::Ready(false)))
                }
                Load::Ready => {
                    self.stop_clock();
                    self.load = Load::Failed;
                    (false, Some(SlotEvent::Failed))
                }
                _ => (false, None),
            };
        }
        if self.info.is_none() {
            if let Some(info) = s.info.clone() {
                self.info = Some(info);
                self.voice = s.voice.clone();
            }
        }
        match self.load {
            Load::Loading => {
                if let Some(i) = s.pictures.iter().position(|p| p.0 == self.generation) {
                    let (_, _, frame) = s.pictures.remove(i).unwrap();
                    if let Some(old) = self.picture.replace(frame) {
                        s.spare.push(old);
                    }
                    wake.notify_all();
                    drop(s);
                    self.load = Load::Ready;
                    return (true, Some(SlotEvent::Ready(true)));
                }
                if s.done || uptime - self.loading_since > LOAD_TIMEOUT {
                    drop(s);
                    self.load = Load::Failed;
                    return (false, Some(SlotEvent::Ready(false)));
                }
                (false, None)
            }
            Load::Ready => {
                if self.started_at.is_none() || self.ended {
                    return (false, None);
                }
                let time = self.clip_ms(uptime);
                // The latest decoded picture that is due: a decoder running
                // late skips ahead instead of slowing the clip down.
                let mut changed = false;
                while let Some(&(generation, ms, _)) = s.pictures.front() {
                    if generation != self.generation {
                        let (_, _, f) = s.pictures.pop_front().unwrap();
                        s.spare.push(f);
                        continue;
                    }
                    if ms as f64 > time + 0.5 {
                        break;
                    }
                    let (_, _, frame) = s.pictures.pop_front().unwrap();
                    if let Some(old) = self.picture.replace(frame) {
                        s.spare.push(old);
                    }
                    changed = true;
                }
                if changed {
                    wake.notify_all();
                }
                let duration = self.info.as_ref().map(|i| i.duration_ms).unwrap_or(0) as f64;
                let finished = s.done && s.pictures.iter().all(|p| p.0 != self.generation);
                drop(s);
                if time >= duration && finished {
                    self.base_ms = duration;
                    self.stop_clock();
                    self.ended = true;
                    return (changed, Some(SlotEvent::Ended));
                }
                (changed, None)
            }
            Load::Idle | Load::Failed => (false, None),
        }
    }
}
