//! The player on real stacked AV1 clips (spec/player.md, "Tests every
//! platform runs", 3): the first clip shows once its first picture is
//! decoded, loops on its length, moves swap in and back; and a canvas read
//! from its URL plays, keeps its files and plays them again offline. The same
//! checks as the Mac's RealVideoTests and CanvasURLTests.

#[path = "delivery_server.rs"]
mod delivery_server;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use delivery_server::{answer_body, app_dir, Reply, Server, CANVAS};
use masko_player_windows::kept::Kept;
use masko_player_windows::{Event, MaskoPlayer, Options};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Two poses on the real clips: `idle` and `wave`, with a move each way on
/// the pointer.
fn delivery(loop_link: &str, move_link: &str) -> String {
    serde_json::json!({
        "deliverySchemaVersion": 1,
        "playerContract": 1,
        "requires": [],
        "target": "windows",
        "mode": "selected",
        "runtimeGraph": {
            "runtimeGraphSchemaVersion": 1,
            "version": "2.1",
            "name": "Real video",
            "initialNode": "idle",
            "autoPlay": true,
            "nodes": [{"id": "idle", "name": "Idle"}, {"id": "wave", "name": "Wave"}],
            "edges": [
                {"id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true,
                 "duration": 0.5, "videos": {"av1": loop_link}},
                {"id": "wave-loop", "source": "wave", "target": "wave", "isLoop": true,
                 "duration": 0.5, "videos": {"av1": loop_link}},
                {"id": "to-wave", "source": "idle", "target": "wave", "isLoop": false,
                 "duration": 0.4, "conditions": [{"input": "behavior::pointerHover"}],
                 "videos": {"av1": move_link}, "movement": {"dx": 10}},
                {"id": "wave-back", "source": "wave", "target": "idle", "isLoop": false,
                 "duration": 0.4,
                 "conditions": [{"input": "behavior::pointerHover", "value": false}],
                 "videos": {"av1": move_link}}
            ]
        }
    })
    .to_string()
}

fn label(event: &Event) -> String {
    match event {
        Event::Pose { pose } => format!("pose:{pose}"),
        Event::Clip { clip, .. } => format!("clip:{clip}"),
        Event::ReactionDone { name, .. } => format!("done:{name}"),
        Event::Movement { edge, .. } => format!("movement:{edge}"),
        Event::Error { code, .. } => format!("error:{code}"),
    }
}

/// A player pumped as an app's window loop would, and what it said.
struct Run {
    player: MaskoPlayer,
    log: Vec<(String, Instant)>,
    pictures: usize,
}

impl Run {
    fn new(player: MaskoPlayer) -> Run {
        Run {
            player,
            log: Vec::new(),
            pictures: 0,
        }
    }

    fn turn(&mut self) {
        if self.player.pump() {
            self.pictures += 1;
        }
        let now = Instant::now();
        for e in self.player.take_events() {
            self.log.push((label(&e), now));
        }
    }

    fn seen(&self, event: &str) -> usize {
        self.log.iter().filter(|(e, _)| e == event).count()
    }

    /// Pumps until `event` was heard `count` times.
    fn wait(&mut self, event: &str, count: usize) {
        let start = Instant::now();
        while self.seen(event) < count {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "{event} x{count} not seen: {:?}",
                self.events()
            );
            self.turn();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn events(&self) -> Vec<String> {
        self.log.iter().map(|(e, _)| e.clone()).collect()
    }
}

fn options(test: &str) -> Options {
    let mut options = Options::new(app_dir(test), (96, 96));
    // Tests leave the computer's sound alone.
    options.audio = false;
    options
}

#[test]
fn real_av1_clips_swap_on_a_decoded_picture_and_end_on_their_length() {
    let loop_file = fixture("loop-av1.mp4");
    let move_file = fixture("move-av1.mp4");
    let json = delivery(loop_file.to_str().unwrap(), move_file.to_str().unwrap());
    let mut run = Run::new(MaskoPlayer::from_delivery(&json, options("player-real")));

    // The first clip shows once its first picture is decoded, then loops on
    // its length, well before the safety net (1.5 s).
    run.wait("clip:idle-loop", 2);
    let shows: Vec<Instant> = run
        .log
        .iter()
        .filter(|(e, _)| e == "clip:idle-loop")
        .map(|(_, at)| *at)
        .collect();
    let looped = shows[1].duration_since(shows[0]).as_secs_f64();
    assert!(looped > 0.4, "looped after {looped} s");
    assert!(
        looped < 1.2,
        "the clip's end, not the safety net, ended it ({looped} s)"
    );
    let frame = run.player.frame().expect("a picture on screen");
    assert_eq!((frame.width, frame.height), (96, 96));
    assert!(run.pictures > 3, "pictures drawn: {}", run.pictures);

    // An instant move swaps to the other slot after its first picture, and back.
    run.player.set_bool("behavior::pointerHover", true, true);
    run.wait("clip:wave-loop", 1);
    let story: Vec<String> = run
        .events()
        .into_iter()
        .filter(|e| !e.starts_with("clip:idle-loop"))
        .collect();
    assert_eq!(
        story,
        [
            "pose:idle",
            "clip:to-wave",
            "movement:to-wave",
            "pose:wave",
            "clip:wave-loop"
        ]
    );
    run.player.set_bool("behavior::pointerHover", false, true);
    run.wait("clip:idle-loop", 3);
    assert!(
        !run.events().iter().any(|e| e.starts_with("error:")),
        "{:?}",
        run.events()
    );
    let position = run.player.position();
    assert_eq!(position.clip.as_deref(), Some("idle-loop"));
    assert!(position.time <= 0.5);
    run.player.destroy();
}

#[test]
fn a_paused_player_holds_its_picture_and_resumes() {
    let json = delivery(
        fixture("loop-av1.mp4").to_str().unwrap(),
        fixture("move-av1.mp4").to_str().unwrap(),
    );
    let mut run = Run::new(MaskoPlayer::from_delivery(&json, options("player-pause")));
    run.wait("clip:idle-loop", 1);
    run.player.pause();
    let shown = run.seen("clip:idle-loop");
    let at = run.player.position().time;
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(900) {
        run.turn();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(run.seen("clip:idle-loop"), shown, "no loop while paused");
    assert_eq!(run.player.position().time, at, "the clip time stood still");
    run.player.play();
    run.wait("clip:idle-loop", shown + 1);
    run.player.destroy();
}

#[test]
fn a_canvas_from_its_url_plays_keeps_its_files_and_plays_them_offline() {
    let server = Server::start();
    let loop_bytes = std::fs::read(fixture("loop-av1.mp4")).unwrap();
    let move_bytes = std::fs::read(fixture("move-av1.mp4")).unwrap();
    server.route("/files/loop-av1.mp4", Reply::ok(loop_bytes));
    server.route("/files/move-av1.mp4", Reply::ok(move_bytes));
    let json = delivery(
        &server.url("/files/loop-av1.mp4?sig=one"),
        &server.url("/files/move-av1.mp4?sig=one"),
    );
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    server.route(CANVAS, Reply::ok(answer_body("release-1", 1, &value)));
    let url = server.url(CANVAS);
    let options = options("player-url");
    let dir = options.app_data_dir.clone();

    // The first delivery plays from its links while every file is kept.
    let mut run = Run::new(MaskoPlayer::from_url(
        &url,
        "masko_app_test",
        options.clone(),
    ));
    run.wait("clip:idle-loop", 2);
    let kept = Kept::new(&dir, &url);
    let start = Instant::now();
    while kept.kept_release().is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "never kept");
        run.turn();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        server.asked("/files/loop-av1.mp4"),
        1,
        "each file downloads once"
    );
    assert_eq!(server.asked("/files/move-av1.mp4"), 1);
    run.player.destroy();
    server.stop();

    // Offline: the kept release plays at once, and the check reports it
    // could not ask.
    let mut offline = Run::new(MaskoPlayer::from_url(&url, "masko_app_test", options));
    offline.wait("clip:idle-loop", 2);
    offline.wait("error:refresh_failed", 1);
    offline.player.destroy();
}
