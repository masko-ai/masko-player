//! The one sound output: every clip voice and edge sound mixed into a single
//! stream (WASAPI on Windows, Core Audio on the Mac, through cpal).
//!
//! A sound is scheduled to be heard at a time on the stream's own clock,
//! never "as soon as possible": `play` returns how long until its first
//! sample reaches the speakers, so the player starts a talking clip's
//! picture then (AV1Slot.swift: "the picture waits for it: the clock starts
//! when the sound reaches the speakers, so a voice stays on the lips, also
//! on Bluetooth"). A voice that the output starts late is cut by the delay
//! instead, so it never falls behind the picture.
//!
//! The stream pauses after a few silent seconds, so a mascot that says
//! nothing never keeps the computer's sound device (and Windows) awake.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use super::Pcm;

/// Time between asking for a sound and the stream mixing it, on top of the
/// output's own latency: covers the wait for the next buffer and a late
/// thread (AV1Slot's `lead` is 50 ms for the same reason).
const LEAD_NS: u64 = 20_000_000;
/// Silent time after which the stream pauses.
const IDLE: Duration = Duration::from_secs(3);

/// One sound being played.
struct Playing {
    id: u64,
    pcm: Pcm,
    volume: f32,
    looping: bool,
    voice: bool,
    rate: f64,
    /// Where it is, in frames of its own sample rate.
    position: f64,
    /// When its first sample is heard, on the stream's clock (ns); None once
    /// it has started.
    starts_at: Option<u64>,
}

/// The mixing itself, without a device: what is playing, and how to fill a
/// buffer with it. The stream's callback runs `render`; tests run it alone.
pub(crate) struct Mixer {
    sounds: Vec<Playing>,
    paused: bool,
    paused_at: u64,
    voices_muted: bool,
    /// From the stream: how long from a callback to its first frame being
    /// heard, and how long a buffer lasts, in ns.
    output_delay: u64,
    period: u64,
    /// Since when the stream has had nothing to play.
    idle_since: Option<Instant>,
}

impl Mixer {
    pub fn new() -> Mixer {
        Mixer {
            sounds: Vec::new(),
            paused: false,
            paused_at: 0,
            voices_muted: false,
            // Until the stream reports its own: a usual shared-mode output.
            output_delay: 20_000_000,
            period: 10_000_000,
            idle_since: Some(Instant::now()),
        }
    }

    /// From asking for a sound now to hearing it, in ns.
    pub fn delay(&self) -> u64 {
        LEAD_NS + self.output_delay + self.period
    }

    /// Adds a sound heard from `from_ms` into it, at `rate`, starting at
    /// `at` on the stream's clock. False when there is nothing to play (it
    /// starts past its end).
    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        id: u64,
        pcm: Pcm,
        volume: f32,
        looping: bool,
        voice: bool,
        from_ms: u64,
        rate: f32,
        at: u64,
    ) -> bool {
        let frames = pcm.frames();
        let rate = if rate.is_finite() && rate > 0.0 {
            rate as f64
        } else {
            1.0
        };
        let mut position = from_ms as f64 / 1000.0 * pcm.sample_rate as f64;
        if frames == 0 || pcm.sample_rate == 0 || (!looping && position >= frames as f64) {
            return false;
        }
        if looping {
            position %= frames as f64;
        }
        self.sounds.push(Playing {
            id,
            pcm,
            volume: volume.clamp(0.0, 4.0),
            looping,
            voice,
            rate,
            position,
            starts_at: Some(at),
        });
        self.idle_since = None;
        true
    }

    pub fn stop(&mut self, id: u64) {
        self.sounds.retain(|s| s.id != id);
    }

    pub fn stop_all(&mut self, only_looping: bool) {
        self.sounds.retain(|s| only_looping && !s.looping);
    }

    pub fn set_voices_muted(&mut self, muted: bool) {
        self.voices_muted = muted;
    }

    /// Pauses or resumes every sound where it is. On resume, a sound that
    /// had started is heard again `delay()` from `now`, from where it
    /// stopped, as a sound just played would be; one still waiting for its
    /// start waits as long again as the pause lasted.
    pub fn set_paused(&mut self, paused: bool, now: u64) {
        if paused == self.paused {
            return;
        }
        if paused {
            self.paused_at = now;
        } else {
            let paused_for = now.saturating_sub(self.paused_at);
            let heard = now + self.delay();
            for sound in &mut self.sounds {
                sound.starts_at = Some(match sound.starts_at {
                    Some(at) => (at + paused_for).max(heard),
                    None => heard,
                });
            }
            self.idle_since = None;
        }
        self.paused = paused;
    }

    /// Nothing can be heard: no sound, or paused.
    pub fn is_silent(&self) -> bool {
        self.paused || self.sounds.is_empty()
    }

    pub fn has(&self, id: u64) -> bool {
        self.sounds.iter().any(|s| s.id == id)
    }

    /// What the stream says of its timing at each callback.
    pub fn note_timing(&mut self, output_delay: u64, period: u64) {
        self.output_delay = output_delay.min(2_000_000_000);
        self.period = period.min(1_000_000_000);
    }

    /// Fills `out` (interleaved, `channels` per frame, at `sample_rate`),
    /// whose first frame is heard at `heard_at` on the stream's clock.
    pub fn render(&mut self, out: &mut [f32], channels: usize, sample_rate: u32, heard_at: u64) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        if self.paused || frames == 0 || sample_rate == 0 {
            self.note_idle();
            return;
        }
        let ns_per_frame = 1e9 / sample_rate as f64;
        let muted = self.voices_muted;
        self.sounds.retain_mut(|sound| {
            let length = sound.pcm.frames() as f64;
            let mut first = 0;
            if let Some(at) = sound.starts_at {
                if at > heard_at {
                    let wait = ((at - heard_at) as f64 / ns_per_frame).round() as usize;
                    if wait >= frames {
                        return true;
                    }
                    first = wait;
                } else if sound.voice {
                    // Heard late: skip what should already have been heard,
                    // so the voice stays with the picture.
                    let late = (heard_at - at) as f64 / 1e9;
                    sound.position += late * sound.pcm.sample_rate as f64 * sound.rate;
                    if sound.looping {
                        sound.position %= length;
                    } else if sound.position >= length {
                        return false;
                    }
                }
                sound.starts_at = None;
            }
            let gain = if sound.voice && muted {
                0.0
            } else {
                sound.volume
            };
            mix(
                sound,
                &mut out[first * channels..],
                channels,
                sample_rate,
                gain,
            )
        });
        self.note_idle();
    }

    fn note_idle(&mut self) {
        if self.is_silent() {
            self.idle_since.get_or_insert_with(Instant::now);
        } else {
            self.idle_since = None;
        }
    }
}

/// Adds `sound` into `out` from its position, resampled linearly to the
/// output's rate times its own rate. False once a one-shot sound has ended.
fn mix(sound: &mut Playing, out: &mut [f32], channels: usize, sample_rate: u32, gain: f32) -> bool {
    let pcm = &sound.pcm;
    let source_channels = pcm.channels.max(1) as usize;
    let length = pcm.frames();
    let step = pcm.sample_rate as f64 / sample_rate as f64 * sound.rate;
    let samples = &pcm.samples;
    for frame in out.chunks_exact_mut(channels) {
        if sound.position >= length as f64 {
            if !sound.looping {
                return false;
            }
            sound.position %= length as f64;
        }
        let i = sound.position as usize;
        let t = (sound.position - i as f64) as f32;
        let j = if i + 1 < length {
            i + 1
        } else if sound.looping {
            0
        } else {
            i
        };
        let at = |frame: usize, channel: usize| {
            samples[frame * source_channels + channel.min(source_channels - 1)]
        };
        let left = at(i, 0) + (at(j, 0) - at(i, 0)) * t;
        let right = at(i, 1) + (at(j, 1) - at(i, 1)) * t;
        if channels == 1 {
            frame[0] += gain * 0.5 * (left + right);
        } else {
            // Left and right on the first two channels; a mono sound on
            // both. Other channels (centre, surround) stay silent.
            frame[0] += gain * left;
            frame[1] += gain * right;
        }
        sound.position += step;
    }
    sound.looping || sound.position < length as f64
}

/// The stream and whether it runs.
struct Output {
    stream: cpal::Stream,
    running: bool,
}

/// What `AudioOut` shares with the thread that pauses an idle stream. The
/// stream's callback only holds the mixer, so dropping `AudioOut` drops the
/// stream.
struct Control {
    output: Mutex<Output>,
    closing: Mutex<bool>,
    wake: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding a lock leaves nothing half done that matters
    // more than going on.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// One output stream that mixes every sound. Send + Sync.
pub struct AudioOut {
    mixer: Arc<Mutex<Mixer>>,
    control: Arc<Control>,
    watcher: Option<JoinHandle<()>>,
}

impl AudioOut {
    /// Opens the default output device. None when there is none (a server,
    /// a test runner, a computer without sound): the player then stays
    /// silent.
    pub fn new() -> Option<AudioOut> {
        let host = cpal::default_host();
        let device = host.default_output_device()?;
        let supported = device.default_output_config().ok()?;
        let mixer = Arc::new(Mutex::new(Mixer::new()));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &supported, mixer.clone()),
            SampleFormat::I16 => build::<i16>(&device, &supported, mixer.clone()),
            SampleFormat::U16 => build::<u16>(&device, &supported, mixer.clone()),
            SampleFormat::I32 => build::<i32>(&device, &supported, mixer.clone()),
            _ => None,
        }?;
        // Running from the start measures the output's latency before the
        // first sound; it pauses after a few silent seconds.
        stream.play().ok()?;
        if let Ok(frames) = stream.buffer_size() {
            let period = frames as u64 * 1_000_000_000 / supported.sample_rate().max(1) as u64;
            let mut mixer = lock(&mixer);
            let delay = mixer.output_delay;
            mixer.note_timing(delay, period);
        }
        let control = Arc::new(Control {
            output: Mutex::new(Output {
                stream,
                running: true,
            }),
            closing: Mutex::new(false),
            wake: Condvar::new(),
        });
        let watcher = {
            let (mixer, control) = (mixer.clone(), control.clone());
            std::thread::Builder::new()
                .name("masko-sound".into())
                .spawn(move || pause_when_idle(&mixer, &control))
                .ok()
        };
        Some(AudioOut {
            mixer,
            control,
            watcher,
        })
    }

    /// The stream, started again if it paused for being idle.
    fn running(&self) -> MutexGuard<'_, Output> {
        let mut output = lock(&self.control.output);
        if !output.running && output.stream.play().is_ok() {
            output.running = true;
        }
        output
    }

    /// Plays `pcm` from `from_ms` into it at `rate` (1 is normal; the pitch
    /// follows the rate), and returns the time in ms until its first sample
    /// reaches the speakers: the picture of a talking clip starts then.
    /// `voice`: a clip's own voice, muted by `set_voices_muted` (it keeps
    /// its place while muted); edge sounds are not voices. Returns 0 when
    /// there is nothing to play (`from_ms` past a one-shot sound's end).
    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &self,
        id: u64,
        pcm: Pcm,
        volume: f32,
        looping: bool,
        voice: bool,
        from_ms: u64,
        rate: f32,
    ) -> u64 {
        let output = self.running();
        let now = output.stream.now().as_nanos() as u64;
        let mut mixer = lock(&self.mixer);
        let delay = mixer.delay();
        if mixer.play(id, pcm, volume, looping, voice, from_ms, rate, now + delay) {
            delay.div_ceil(1_000_000)
        } else {
            0
        }
    }

    /// Stops every sound played with this id.
    pub fn stop(&self, id: u64) {
        lock(&self.mixer).stop(id);
    }

    /// Stops the looping sounds, or every sound (`stopSounds`).
    pub fn stop_all(&self, only_looping: bool) {
        lock(&self.mixer).stop_all(only_looping);
    }

    /// Whether a sound with this id is still playing or waiting to start.
    pub fn is_playing(&self, id: u64) -> bool {
        lock(&self.mixer).has(id)
    }

    /// The app's `sound` off: clip voices go silent; edge sounds play on.
    pub fn set_voices_muted(&self, muted: bool) {
        lock(&self.mixer).set_voices_muted(muted);
    }

    /// The player paused or resumed: every sound holds where it is. Resumed
    /// sounds are heard again `latency_ms()` later, from where they stopped:
    /// the picture of a talking clip resumes then, as after `play`.
    pub fn set_paused(&self, paused: bool) {
        let output = if paused {
            lock(&self.control.output)
        } else {
            self.running()
        };
        let now = output.stream.now().as_nanos() as u64;
        lock(&self.mixer).set_paused(paused, now);
    }

    /// From asking for a sound now to hearing it, in ms: a short lead, one
    /// buffer and the output's own latency (long on Bluetooth). What `play`
    /// returns, and what AV1Slot calls the sound's `delay`: a voice turned on
    /// in the middle of a clip starts at `clip time + latency_ms() * rate`.
    pub fn latency_ms(&self) -> u64 {
        lock(&self.mixer).delay().div_ceil(1_000_000)
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        *lock(&self.control.closing) = true;
        self.control.wake.notify_all();
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

/// Pauses the stream once it has had nothing to play for `IDLE`, until
/// `AudioOut` is dropped.
fn pause_when_idle(mixer: &Mutex<Mixer>, control: &Control) {
    loop {
        {
            let closing = lock(&control.closing);
            if *closing {
                return;
            }
            let (closing, _) = control
                .wake
                .wait_timeout(closing, Duration::from_millis(500))
                .unwrap_or_else(|e| e.into_inner());
            if *closing {
                return;
            }
        }
        let mut output = lock(&control.output);
        if output.running {
            let idle = {
                let mixer = lock(mixer);
                mixer.is_silent() && mixer.idle_since.is_some_and(|t| t.elapsed() >= IDLE)
            };
            if idle && output.stream.pause().is_ok() {
                output.running = false;
            }
        }
    }
}

/// Opens the stream in the device's own format; the callback mixes in f32
/// and converts.
fn build<T>(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    mixer: Arc<Mutex<Mixer>>,
) -> Option<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let config = supported.config();
    let channels = (config.channels as usize).max(1);
    let rate = config.sample_rate;
    let mut mixed: Vec<f32> = Vec::new();
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let heard_at = timestamp.playback.as_nanos() as u64;
                let output_delay = timestamp
                    .playback
                    .as_nanos()
                    .saturating_sub(timestamp.callback.as_nanos())
                    as u64;
                let period = (data.len() / channels) as u64 * 1_000_000_000 / rate.max(1) as u64;
                if mixed.len() < data.len() {
                    mixed.resize(data.len(), 0.0);
                }
                let mixed = &mut mixed[..data.len()];
                {
                    let mut mixer = lock(&mixer);
                    mixer.note_timing(output_delay, period);
                    mixer.render(mixed, channels, rate, heard_at);
                }
                for (out, &sample) in data.iter_mut().zip(mixed.iter()) {
                    *out = T::from_sample(sample.clamp(-1.0, 1.0));
                }
            },
            // A lost device leaves the player silent; nothing to tell.
            |_| {},
            None,
        )
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// A mono sound at 1 kHz whose sample n is n + 1.
    fn ramp(frames: usize) -> Pcm {
        Pcm {
            sample_rate: 1000,
            channels: 1,
            samples: (0..frames)
                .map(|n| (n + 1) as f32)
                .collect::<Vec<_>>()
                .into(),
        }
    }

    /// Renders `frames` stereo frames at 1 kHz heard from `heard_at`.
    fn render(mixer: &mut Mixer, frames: usize, heard_at: u64) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        mixer.render(&mut out, 2, 1000, heard_at);
        out.chunks(2).map(|f| f[0]).collect()
    }

    #[test]
    fn a_sound_starts_at_its_time_on_the_stream_clock() {
        let mut mixer = Mixer::new();
        assert!(mixer.play(1, ramp(10), 1.0, false, false, 0, 1.0, 1003 * MS));
        // The buffer heard from 1000 ms holds three silent frames first.
        assert_eq!(
            render(&mut mixer, 6, 1000 * MS),
            [0.0, 0.0, 0.0, 1.0, 2.0, 3.0]
        );
        assert_eq!(render(&mut mixer, 4, 1006 * MS), [4.0, 5.0, 6.0, 7.0]);
        // It ends after its last frame and leaves the mixer.
        assert_eq!(render(&mut mixer, 5, 1010 * MS), [8.0, 9.0, 10.0, 0.0, 0.0]);
        assert!(mixer.is_silent());
    }

    #[test]
    fn a_sound_due_in_a_later_buffer_waits() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(4), 1.0, false, false, 0, 1.0, 20 * MS);
        assert_eq!(render(&mut mixer, 10, 0), [0.0; 10]);
        assert_eq!(render(&mut mixer, 10, 10 * MS), [0.0; 10]);
        assert_eq!(render(&mut mixer, 3, 20 * MS), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_late_voice_skips_ahead_and_a_late_edge_sound_does_not() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(10), 1.0, false, true, 0, 1.0, 100 * MS);
        mixer.play(2, ramp(10), 0.5, false, false, 0, 1.0, 100 * MS);
        // The output starts 3 ms late: the voice is 3 frames in, the edge
        // sound starts from its beginning.
        assert_eq!(render(&mut mixer, 2, 103 * MS), [4.0 + 0.5, 5.0 + 1.0]);
    }

    #[test]
    fn from_ms_and_rate() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(20), 1.0, false, true, 5, 2.0, 0);
        assert_eq!(render(&mut mixer, 3, 0), [6.0, 8.0, 10.0]);
        // Half speed interpolates between samples.
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(20), 1.0, false, true, 0, 0.5, 0);
        assert_eq!(render(&mut mixer, 4, 0), [1.0, 1.5, 2.0, 2.5]);
        // Past a one-shot sound's end there is nothing to play.
        assert!(!Mixer::new().play(1, ramp(20), 1.0, false, true, 20, 1.0, 0));
    }

    #[test]
    fn a_looping_sound_wraps_without_a_gap_until_stopped() {
        let mut mixer = Mixer::new();
        mixer.play(7, ramp(3), 1.0, true, false, 0, 1.0, 0);
        assert_eq!(
            render(&mut mixer, 7, 0),
            [1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0]
        );
        mixer.stop_all(false);
        assert!(mixer.is_silent());
    }

    #[test]
    fn stop_by_id_and_stop_only_looping() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(100), 1.0, true, false, 0, 1.0, 0);
        mixer.play(2, ramp(100), 1.0, false, false, 0, 1.0, 0);
        mixer.play(3, ramp(100), 1.0, false, false, 0, 1.0, 0);
        mixer.stop(3);
        assert!(!mixer.has(3));
        mixer.stop_all(true);
        assert!(!mixer.has(1));
        assert!(mixer.has(2));
    }

    #[test]
    fn muted_voices_are_silent_but_keep_their_place() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(10), 1.0, false, true, 0, 1.0, 0);
        mixer.play(2, ramp(10), 1.0, false, false, 0, 1.0, 0);
        mixer.set_voices_muted(true);
        // Only the edge sound is heard.
        assert_eq!(render(&mut mixer, 2, 0), [1.0, 2.0]);
        mixer.set_voices_muted(false);
        assert_eq!(render(&mut mixer, 2, 2 * MS), [3.0 + 3.0, 4.0 + 4.0]);
    }

    #[test]
    fn pause_holds_every_sound_and_delays_those_waiting() {
        let mut mixer = Mixer::new();
        mixer.play(1, ramp(10), 1.0, false, false, 0, 1.0, 0);
        mixer.play(2, ramp(10), 1.0, false, false, 0, 1.0, 50 * MS);
        assert_eq!(render(&mut mixer, 2, 0), [1.0, 2.0]);
        mixer.set_paused(true, 2 * MS);
        assert!(mixer.is_silent());
        assert_eq!(render(&mut mixer, 3, 2 * MS), [0.0; 3]);
        // Resumed 100 ms later: the first is heard again after the delay
        // from where it was, the second starts 100 ms later than it would
        // have.
        mixer.note_timing(0, 0);
        mixer.set_paused(false, 102 * MS);
        let heard = (102 * MS + mixer.delay()) / MS;
        assert_eq!(render(&mut mixer, 2, 102 * MS), [0.0, 0.0]);
        assert_eq!(render(&mut mixer, 2, heard * MS), [3.0, 4.0]);
        mixer.stop(1);
        assert_eq!(render(&mut mixer, 1, 149 * MS), [0.0]);
        assert_eq!(render(&mut mixer, 1, 150 * MS), [1.0]);
    }

    #[test]
    fn volume_and_stereo_layout() {
        let mut mixer = Mixer::new();
        let stereo = Pcm {
            sample_rate: 1000,
            channels: 2,
            samples: vec![0.2, -0.4, 0.6, -0.8].into(),
        };
        mixer.play(1, stereo, 0.5, false, false, 0, 1.0, 0);
        let mut out = vec![0.0; 2 * 3];
        mixer.render(&mut out, 3, 1000, 0);
        assert_eq!(out, [0.1, -0.2, 0.0, 0.3, -0.4, 0.0]);
    }

    #[test]
    fn the_delay_is_the_lead_plus_the_reported_latency() {
        let mut mixer = Mixer::new();
        mixer.note_timing(30 * MS, 10 * MS);
        assert_eq!(mixer.delay(), LEAD_NS + 40 * MS);
    }

    #[test]
    fn audio_out_is_send_and_sync() {
        fn shareable<T: Send + Sync>() {}
        shareable::<AudioOut>();
    }
}
