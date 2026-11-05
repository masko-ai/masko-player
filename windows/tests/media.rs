//! The media layer against real files (spec/player.md, "Tests every
//! platform runs", 3): stacked AV1 clips decoded with dav1d and drawn
//! transparent, their voice, edge sounds and pose stills.
//!
//! loop-av1.mp4 and move-av1.mp4 are the Apple tests' clips: 96x192 stacked
//! AV1, a still orange (#F95D02) disc on transparency, 15 and 12 frames at
//! 30 fps; the loop has an AAC voice. slide-av1.mp4 (64x128, BT.709 tagged
//! like Masko's files) has an orange square that moves 3 pixels a frame
//! from x = 2, for 12 frames. tick.mp3 and tick.m4a are 0.25 s tones at
//! amplitude 1/16. All made with ffmpeg.

use std::path::{Path, PathBuf};

use masko_player_windows::media::{
    clip_audio, decode_still, sound_file, AudioOut, ClipFile, Frame, MediaError, Pcm, VideoDecoder,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A file in a folder of its own under the system's temporary folder.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("masko-media-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn close(value: u8, expected: u8, tolerance: u8) -> bool {
    value.abs_diff(expected) <= tolerance
}

/// Every colour channel of a premultiplied pixel is at most its alpha.
fn premultiplied(frame: &Frame) -> bool {
    frame
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
}

fn rms(pcm: &Pcm) -> f32 {
    (pcm.samples.iter().map(|s| s * s).sum::<f32>() / pcm.samples.len() as f32).sqrt()
}

#[test]
fn a_stacked_clip_is_read_without_decoding() {
    let clip = ClipFile::open(&fixture("loop-av1.mp4")).unwrap();
    let info = clip.info();
    assert_eq!(info.duration_ms, 500);
    assert_eq!(info.frame_count, 15);
    assert!(info.has_audio);
    assert_eq!(
        (info.width, info.height),
        (96, 96),
        "the visible picture is the top half"
    );
    assert_eq!(info.frame_times_ms.len(), 15);
    assert_eq!(info.frame_times_ms[0], 0);
    assert_eq!(info.frame_times_ms[1], 33);
    assert_eq!(info.frame_times_ms[14], 467);
    // The same frames as AV1Clip.frameIndex in the Apple tests.
    assert_eq!(info.frame_at(0), 0);
    assert_eq!(info.frame_at(250), 7);
    assert_eq!(info.frame_at(9000), 14);

    let silent = ClipFile::open(&fixture("move-av1.mp4")).unwrap();
    assert_eq!(silent.info().duration_ms, 400);
    assert_eq!(silent.info().frame_count, 12);
    assert!(!silent.info().has_audio);
}

#[test]
fn files_that_are_not_stacked_av1_clips_are_refused() {
    assert!(matches!(
        ClipFile::open(&fixture("tick.m4a")),
        Err(MediaError::Format(_))
    ));
    assert!(matches!(
        ClipFile::open(&fixture("tick.mp3")),
        Err(MediaError::Format(_))
    ));
    assert!(matches!(
        ClipFile::open(&fixture("missing.mp4")),
        Err(MediaError::Io(_))
    ));
}

#[test]
fn frames_decode_transparent_and_premultiplied() {
    let clip = ClipFile::open(&fixture("loop-av1.mp4")).unwrap();
    let mut decoder = VideoDecoder::new(&clip, 96, 96).unwrap();
    let mut frame = Frame::new(1, 1);
    assert_eq!(decoder.next(&mut frame).unwrap(), Some(0));
    assert_eq!(
        (frame.width, frame.height),
        (96, 96),
        "resized to the output size"
    );
    assert_eq!(
        frame.bgra(2, 2)[3],
        0,
        "transparent where the clip is transparent"
    );
    assert_eq!(frame.bgra(2, 2), [0, 0, 0, 0]);
    let [b, g, r, a] = frame.bgra(48, 48);
    assert_eq!(a, 255, "opaque where the clip is opaque");
    // #F95D02, give or take the encoding, as the Apple test checks.
    assert!(
        close(r, 0xF9, 12) && close(g, 0x5D, 12) && close(b, 0x02, 12),
        "{:?}",
        [b, g, r, a]
    );
    assert!(premultiplied(&frame));

    // Every frame, in order, at its time, then the end.
    let mut times = vec![0];
    while let Some(time) = decoder.next(&mut frame).unwrap() {
        assert!(premultiplied(&frame));
        times.push(time);
    }
    assert_eq!(times, clip.info().frame_times_ms);
    assert_eq!(
        decoder.next(&mut frame).unwrap(),
        None,
        "the end stays the end"
    );
}

#[test]
fn frames_come_in_order_and_move() {
    let clip = ClipFile::open(&fixture("slide-av1.mp4")).unwrap();
    assert_eq!(clip.info().frame_count, 12);
    assert_eq!((clip.info().width, clip.info().height), (64, 64));
    let mut decoder = VideoDecoder::new(&clip, 64, 64).unwrap();
    let mut frame = Frame::new(64, 64);
    for k in 0..12u32 {
        let time = decoder.next(&mut frame).unwrap();
        assert_eq!(time, Some(clip.info().frame_times_ms[k as usize]));
        // The square's centre, and 4 pixels left of the square.
        let centre = frame.bgra(10 + 3 * k, 32);
        assert!(centre[3] >= 240, "frame {k}: {centre:?}");
        assert!(
            close(centre[2], 0xF9, 16) && close(centre[1], 0x5D, 16),
            "frame {k}: {centre:?}"
        );
        if k > 0 {
            let outside = frame.bgra(3 * k - 2, 32);
            assert!(outside[3] <= 16, "frame {k}: {outside:?}");
        }
    }
    assert_eq!(decoder.next(&mut frame).unwrap(), None);
}

#[test]
fn rewind_starts_the_same_clip_again() {
    let clip = ClipFile::open(&fixture("slide-av1.mp4")).unwrap();
    let mut decoder = VideoDecoder::new(&clip, 64, 64).unwrap();
    let mut first = Frame::new(64, 64);
    decoder.next(&mut first).unwrap();
    let mut frame = Frame::new(64, 64);
    for _ in 0..5 {
        decoder.next(&mut frame).unwrap();
    }
    assert_ne!(frame, first, "the clip moves");
    for round in 0..3 {
        decoder.rewind().unwrap();
        assert_eq!(decoder.next(&mut frame).unwrap(), Some(0));
        assert_eq!(frame, first, "frame 0 again after rewind {round}");
        // Rewinding before the end, and at the end, both work.
        if round == 1 {
            while decoder.next(&mut frame).unwrap().is_some() {}
        }
    }
    // Skipping decodes without drawing.
    decoder.rewind().unwrap();
    assert_eq!(decoder.skip().unwrap(), Some(0));
    assert_eq!(decoder.skip().unwrap(), Some(33));
}

#[test]
fn pictures_scale_to_any_size() {
    let clip = ClipFile::open(&fixture("move-av1.mp4")).unwrap();
    for (w, h) in [(40, 40), (200, 200), (96, 48), (1, 1)] {
        let mut decoder = VideoDecoder::new(&clip, w, h).unwrap();
        let mut frame = Frame::new(w, h);
        assert_eq!(decoder.next(&mut frame).unwrap(), Some(0));
        assert_eq!((frame.width, frame.height), (w, h));
        assert!(premultiplied(&frame));
        if w >= 40 {
            assert_eq!(frame.bgra(0, 0)[3], 0, "{w}x{h}: corner");
            assert_eq!(frame.bgra(w / 2, h / 2)[3], 255, "{w}x{h}: centre");
        }
    }
    assert!(VideoDecoder::new(&clip, 0, 10).is_err());
}

#[test]
fn the_clip_voice_starts_with_the_first_frame() {
    let clip = ClipFile::open(&fixture("loop-av1.mp4")).unwrap();
    let voice = clip_audio(&clip).unwrap().expect("the loop has a voice");
    assert_eq!(voice.sample_rate, 44100);
    assert_eq!(voice.channels, 1);
    // The file's edit list plays 0.5 s after 1024 frames of AAC priming:
    // exactly that is kept.
    assert_eq!(voice.frames(), 22050);
    assert_eq!(voice.duration_ms(), 500);
    assert!(rms(&voice) > 0.03, "it is not silence: {}", rms(&voice));

    let silent = ClipFile::open(&fixture("move-av1.mp4")).unwrap();
    assert!(clip_audio(&silent).unwrap().is_none());
}

#[test]
fn edge_sounds_decode_from_mp3_m4a_and_wav() {
    let mp3 = sound_file(&fixture("tick.mp3")).unwrap();
    assert_eq!((mp3.sample_rate, mp3.channels), (44100, 1));
    // Gapless: the encoder's delay and padding are cut, so a looping
    // sound loops without a gap.
    assert!(mp3.duration_ms().abs_diff(250) <= 5, "{mp3:?}");
    assert!(
        (rms(&mp3) - 0.0442).abs() < 0.005,
        "a 1/16 sine: {}",
        rms(&mp3)
    );

    let m4a = sound_file(&fixture("tick.m4a")).unwrap();
    assert_eq!((m4a.sample_rate, m4a.channels), (48000, 2));
    assert_eq!(m4a.frames(), 12000);
    // ffmpeg made it stereo with each side 3 dB lower.
    assert!((rms(&m4a) - 0.0313).abs() < 0.003, "{}", rms(&m4a));

    // A 16-bit WAV, written here.
    let samples: Vec<i16> = (0..100).map(|n| (n * 100) as i16).collect();
    let mut wav = Vec::new();
    let data_len = samples.len() as u32 * 2;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&8000u32.to_le_bytes());
    wav.extend_from_slice(&16000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in &samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    let path = scratch("tone.wav");
    std::fs::write(&path, wav).unwrap();
    let pcm = sound_file(&path).unwrap();
    assert_eq!(
        (pcm.sample_rate, pcm.channels, pcm.frames()),
        (8000, 1, 100)
    );
    assert!((pcm.samples[50] - 5000.0 / 32768.0).abs() < 1e-4);

    // Not a sound.
    let text = scratch("notes.txt");
    std::fs::write(&text, "not a sound at all, just some words in a file").unwrap();
    assert!(matches!(sound_file(&text), Err(MediaError::Format(_))));
}

fn write_png(
    path: &Path,
    width: u32,
    height: u32,
    color: png::ColorType,
    data: &[u8],
    palette: Option<(&[u8], &[u8])>,
) {
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    if let Some((rgb, alpha)) = palette {
        encoder.set_palette(rgb.to_vec());
        encoder.set_trns(alpha.to_vec());
    }
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(data).unwrap();
}

#[test]
fn pose_stills_scale_premultiplied() {
    // 8x8: opaque red on the left half, transparent green on the right.
    let mut rgba = Vec::new();
    for _y in 0..8 {
        for x in 0..8 {
            rgba.extend_from_slice(if x < 4 {
                &[255, 0, 0, 255]
            } else {
                &[0, 255, 0, 0]
            });
        }
    }
    let path = scratch("pose.png");
    write_png(&path, 8, 8, png::ColorType::Rgba, &rgba, None);

    let same = decode_still(&path, 8, 8).unwrap();
    assert_eq!(same.bgra(0, 0), [0, 0, 255, 255]);
    assert_eq!(same.bgra(7, 7), [0, 0, 0, 0]);

    let small = decode_still(&path, 2, 2).unwrap();
    assert!(premultiplied(&small));
    let left = small.bgra(0, 0);
    let right = small.bgra(1, 0);
    assert!(left[3] > 200 && right[3] < 60, "{left:?} {right:?}");
    // The transparent green never bleeds into the edge.
    assert_eq!(right[1], 0, "{right:?}");
    assert_eq!(left[1], 0, "{left:?}");

    let big = decode_still(&path, 64, 32).unwrap();
    assert_eq!((big.width, big.height), (64, 32));
    assert_eq!(big.bgra(2, 16), [0, 0, 255, 255]);
    assert_eq!(big.bgra(62, 16), [0, 0, 0, 0]);

    // A palette with a transparent entry.
    let indexed = scratch("indexed.png");
    write_png(
        &indexed,
        2,
        1,
        png::ColorType::Indexed,
        &[0, 1],
        Some((&[0, 0, 255, 9, 9, 9], &[255, 0])),
    );
    let still = decode_still(&indexed, 2, 1).unwrap();
    assert_eq!(still.bgra(0, 0), [255, 0, 0, 255]);
    assert_eq!(still.bgra(1, 0), [0, 0, 0, 0]);

    assert!(matches!(
        decode_still(&fixture("tick.mp3"), 8, 8),
        Err(MediaError::Format(_))
    ));
}

#[test]
fn the_sound_output_opens_when_there_is_a_device() {
    // A computer without sound gives None, and the player stays silent.
    let Some(out) = AudioOut::new() else {
        return;
    };
    let latency = out.latency_ms();
    assert!(latency > 0 && latency < 2000, "{latency}");
    // Silent, so running the tests makes no noise.
    let pcm = sound_file(&fixture("tick.mp3")).unwrap();
    let delay = out.play(1, pcm.clone(), 0.0, true, true, 0, 1.0);
    assert!(
        delay.abs_diff(out.latency_ms()) <= 5,
        "{delay} {}",
        out.latency_ms()
    );
    assert!(out.is_playing(1));
    assert_eq!(
        out.play(2, pcm, 0.0, false, false, 10_000, 1.0),
        0,
        "past its end"
    );
    out.set_voices_muted(true);
    out.set_paused(true);
    out.set_paused(false);
    out.stop_all(true);
    assert!(!out.is_playing(1));
}

/// How fast a real clip decodes and composes: `MASKO_BENCH_CLIP=<a stacked
/// AV1 MP4> cargo test --release --test media -- --ignored --nocapture`.
#[test]
#[ignore]
fn speed() {
    let Some(path) = std::env::var_os("MASKO_BENCH_CLIP") else {
        eprintln!("set MASKO_BENCH_CLIP to a stacked AV1 clip");
        return;
    };
    let clip = ClipFile::open(Path::new(&path)).unwrap();
    let info = clip.info().clone();
    eprintln!(
        "{}x{} visible, {} frames, {} ms, voice: {}",
        info.width, info.height, info.frame_count, info.duration_ms, info.has_audio
    );
    let started = std::time::Instant::now();
    let voice = clip_audio(&clip).unwrap();
    eprintln!("voice decoded in {:?}: {voice:?}", started.elapsed());
    for (w, h) in [
        (info.width, info.height),
        (info.width / 2, info.height / 2),
        (180, 180),
    ] {
        let mut decoder = VideoDecoder::new(&clip, w, h).unwrap();
        let mut frame = Frame::new(w, h);
        // The best of three runs, each over the whole clip: decoding alone,
        // then decoding and composing.
        let mut decode = f64::MAX;
        let mut total = f64::MAX;
        for _ in 0..3 {
            decoder.rewind().unwrap();
            let started = std::time::Instant::now();
            let mut n = 0;
            while decoder.skip().unwrap().is_some() {
                n += 1;
            }
            decode = decode.min(started.elapsed().as_secs_f64() * 1000.0 / n as f64);
            decoder.rewind().unwrap();
            let started = std::time::Instant::now();
            let mut n = 0;
            while decoder.next(&mut frame).unwrap().is_some() {
                n += 1;
            }
            total = total.min(started.elapsed().as_secs_f64() * 1000.0 / n as f64);
        }
        eprintln!(
            "{w}x{h}: {total:.2} ms a frame ({decode:.2} decoding, {:.2} composing)",
            total - decode
        );
    }
}

#[test]
fn the_types_go_to_other_threads() {
    fn send<T: Send>() {}
    fn shared<T: Send + Sync>() {}
    send::<VideoDecoder>();
    shared::<ClipFile>();
    shared::<Pcm>();
    shared::<AudioOut>();
    shared::<Frame>();
}
