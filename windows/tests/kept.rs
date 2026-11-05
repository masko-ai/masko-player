//! A canvas's release kept on the computer (src/kept.rs), against a local
//! server: what is downloaded, when the release is kept, and what plays
//! offline.

#[path = "delivery_server.rs"]
mod server;

use masko_player_windows::delivery::{
    edge_media, fetch, links, pose_stills, same_release, Fetched, TARGET,
};
use masko_player_windows::kept::{KeepError, Kept};
use serde_json::Value;
use server::{app_dir, delivery, file_body, serve, Reply, Server, CANVAS};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const KEY: &str = "masko_app_test_secret";

fn sha(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A server with a release and its files, and that release as fetched.
fn released(server: &Server, sig: &str, fingerprint: &str, number: u64) -> Fetched {
    let d = delivery(&server.url(""), sig, fingerprint);
    serve(server, &format!("rel-{number}"), number, &d);
    fetch(&server.url(CANVAS), KEY, "install", TARGET).unwrap()
}

fn files_in(kept: &Kept) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(kept.folder().join("files"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn files_are_kept_per_canvas_and_named_after_their_link_path() {
    let app = app_dir("names");
    let kept = Kept::new(
        &app,
        "https://api.masko.ai/v1/canvases/c1/delivery?target=windows&formats=av1",
    );
    let root = std::path::absolute(&app).unwrap().join("MaskoPlayer");
    // The URL without its query: the same canvas keeps its files whatever it
    // is asked with.
    assert_eq!(
        kept.folder(),
        root.join(&sha("https://api.masko.ai/v1/canvases/c1/delivery")[..16])
    );
    assert_eq!(
        Kept::new(&app, "https://api.masko.ai/v1/canvases/c1/delivery").folder(),
        kept.folder()
    );
    assert_ne!(
        Kept::new(&app, "https://api.masko.ai/v1/canvases/c2/delivery").folder(),
        kept.folder()
    );

    let file = kept.file_for("https://cdn.example.invalid/m/loop-idle.mp4?sig=s1&exp=1");
    assert_eq!(
        file,
        kept.folder()
            .join("files")
            .join(format!("{}.mp4", sha("/m/loop-idle.mp4")))
    );
    // Fresh signatures and another host keep the same file.
    assert_eq!(
        kept.file_for("https://other.example.invalid/m/loop-idle.mp4?sig=s2"),
        file
    );
    // The path is read percent-decoded, as on the Mac.
    assert_eq!(
        kept.file_for("https://cdn.example.invalid/m/a%20b.png"),
        kept.folder()
            .join("files")
            .join(format!("{}.png", sha("/m/a b.png")))
    );
    // No extension, or one Windows cannot name, is `bin`.
    assert_eq!(
        kept.file_for("https://cdn.example.invalid/m/sound"),
        kept.folder()
            .join("files")
            .join(format!("{}.bin", sha("/m/sound")))
    );
    assert!(kept
        .file_for("https://cdn.example.invalid/m/x.a%3Ab")
        .to_string_lossy()
        .ends_with(".bin"));
}

#[test]
fn keep_downloads_every_file_then_writes_the_release() {
    let server = Server::start();
    let app = app_dir("keep");
    let fetched = released(&server, "s1", "fingerprint-1", 1);
    let kept = Kept::new(&app, &server.url(CANVAS));
    assert!(kept.kept_release().is_none());

    let local = kept.keep(&fetched).unwrap();

    assert_eq!(local.release_id, "rel-1");
    assert_eq!(local.number, 1);
    let remote = links(&fetched.delivery_json).unwrap();
    assert_eq!(files_in(&kept).len(), remote.len());
    for link in &remote {
        let file = kept.file_for(link);
        let name = link.split('?').next().unwrap().rsplit('/').next().unwrap();
        assert_eq!(fs::read(&file).unwrap(), file_body(name), "{link}");
        let path = link.split("127.0.0.1").nth(1).unwrap();
        let path = path[path.find('/').unwrap()..].split('?').next().unwrap();
        assert_eq!(server.asked(path), 1, "{path} downloaded once");
    }
    // Only what the Windows player plays is downloaded.
    assert_eq!(server.asked("/files/loop-idle.mov"), 0);
    assert_eq!(server.asked("/files/loop-idle.webm"), 0);
    // The canvas URL itself is asked once, by `fetch`; files carry no key.
    assert_eq!(server.asked(CANVAS), 1);
    assert!(server
        .requests()
        .iter()
        .filter(|r| r.target.starts_with("/files/"))
        .all(|r| !r.headers.contains_key("authorization")));
    assert!(kept.folder().join("release.json").is_file());

    // The release to play reads files from disk only.
    for media in edge_media(&local.delivery_json).unwrap().values() {
        for file in media
            .video
            .iter()
            .chain(media.variants.iter().map(|(_, f)| f))
        {
            assert!(
                Path::new(file).is_absolute() && Path::new(file).is_file(),
                "{file}"
            );
        }
        if let Some(sound) = &media.sound {
            assert!(Path::new(&sound.url).is_file());
        }
    }
    for still in pose_stills(&local.delivery_json).unwrap().values() {
        assert!(Path::new(still).is_file(), "{still}");
    }
    let local_links = links(&local.delivery_json).unwrap();
    assert_eq!(local_links.len(), remote.len());
    assert!(local_links.iter().all(|l| !l.starts_with("http")));
    // Links the player never plays stay as they were.
    let local_value: Value = serde_json::from_str(&local.delivery_json).unwrap();
    assert!(local_value["runtimeGraph"]["edges"][0]["videos"]["hevc"]
        .as_str()
        .unwrap()
        .starts_with("http://127.0.0.1"));
    // The engine plays it, and it is the release that was fetched.
    assert!(masko_player_engine::load(&local.delivery_json).is_ok());
    assert!(same_release(&local.delivery_json, &fetched.delivery_json));
    assert!(same_release(&fetched.delivery_json, &local.delivery_json));
}

#[test]
fn the_kept_release_plays_offline_at_the_next_launch() {
    let server = Server::start();
    let app = app_dir("offline");
    let url = server.url(CANVAS);
    let fetched = released(&server, "s1", "fingerprint-1", 3);
    let first = Kept::new(&app, &url).keep(&fetched).unwrap();
    server.stop();

    // A new launch, without a server.
    let kept = Kept::new(&app, &url);
    kept.remove_unused();
    let release = kept.kept_release().unwrap();
    assert_eq!(release.release_id, "rel-3");
    assert_eq!(release.number, 3);
    assert_eq!(release.delivery_json, first.delivery_json);
    assert!(masko_player_engine::load(&release.delivery_json).is_ok());
    for link in links(&release.delivery_json).unwrap() {
        assert!(Path::new(&link).is_file(), "{link}");
    }
    // Every file is still there.
    assert_eq!(
        files_in(&kept).len(),
        links(&fetched.delivery_json).unwrap().len()
    );

    // A missing file means no kept release.
    let some = links(&release.delivery_json).unwrap().remove(0);
    fs::remove_file(some).unwrap();
    assert!(kept.kept_release().is_none());
}

#[test]
fn a_file_cut_short_is_not_kept_nor_the_release() {
    let server = Server::start();
    let app = app_dir("cut");
    let fetched = released(&server, "s1", "fingerprint-1", 1);
    server.route(
        "/files/loop-idle.mp4",
        Reply::cut(file_body("loop-idle.mp4")),
    );
    let kept = Kept::new(&app, &server.url(CANVAS));
    let link = links(&fetched.delivery_json)
        .unwrap()
        .into_iter()
        .find(|l| l.contains("/loop-idle.mp4"))
        .unwrap();

    let error = kept.keep(&fetched).unwrap_err();
    assert!(matches!(error, KeepError::Network(_)), "{error:?}");
    assert!(error.to_string().contains("loop-idle.mp4"));
    assert!(!error.to_string().contains("sig="), "{error}");
    assert!(!kept.file_for(&link).exists());
    assert!(
        files_in(&kept).iter().all(|name| !name.ends_with(".part")),
        "{:?}",
        files_in(&kept)
    );
    assert!(!kept.folder().join("release.json").exists());
    assert!(kept.kept_release().is_none());

    // Empty files and error answers are not kept either.
    server.route("/files/loop-idle.mp4", Reply::ok(Vec::new()));
    assert!(matches!(kept.keep(&fetched), Err(KeepError::Network(_))));
    server.route("/files/loop-idle.mp4", Reply::status(403, "expired"));
    assert!(matches!(kept.keep(&fetched), Err(KeepError::Network(_))));
    assert!(!kept.file_for(&link).exists());
    assert!(files_in(&kept).iter().all(|name| !name.ends_with(".part")));

    // Once it comes whole, only the missing file is downloaded again.
    server.route(
        "/files/loop-idle.mp4",
        Reply::ok(file_body("loop-idle.mp4")),
    );
    let before = server.asked("/files/idle-to-alert.mp4");
    kept.keep(&fetched).unwrap();
    assert_eq!(server.asked("/files/idle-to-alert.mp4"), before);
    assert_eq!(
        fs::read(kept.file_for(&link)).unwrap(),
        file_body("loop-idle.mp4")
    );
    assert!(kept.kept_release().is_some());
}

#[test]
fn a_newer_release_takes_over_only_once_kept_whole() {
    let server = Server::start();
    let app = app_dir("newer");
    let url = server.url(CANVAS);
    let kept = Kept::new(&app, &url);
    let first = released(&server, "s1", "fingerprint-1", 1);
    kept.keep(&first).unwrap();
    let old_files = files_in(&kept);

    // The newer release has other files (another path); one fails.
    let mut newer = delivery(&server.url("/v2"), "s9", "fingerprint-2");
    newer["runtimeGraph"]["edges"][0]["duration"] = serde_json::json!(5);
    serve(&server, "rel-2", 2, &newer);
    let newer_links = links(&newer.to_string()).unwrap();
    let broken = newer_links[3].clone();
    let broken_path = broken.split("127.0.0.1").nth(1).unwrap();
    let broken_path = broken_path[broken_path.find('/').unwrap()..]
        .split('?')
        .next()
        .unwrap()
        .to_owned();
    server.route(&broken_path, Reply::cut(file_body("x")));
    let fetched = fetch(&url, KEY, "install", TARGET).unwrap();
    assert_eq!(fetched.number, 2);
    assert!(!same_release(&fetched.delivery_json, &first.delivery_json));

    assert!(kept.keep(&fetched).is_err());
    // The release kept before still plays, whole.
    assert_eq!(kept.kept_release().unwrap().release_id, "rel-1");

    server.route(&broken_path, Reply::ok(file_body("x")));
    let local = kept.keep(&fetched).unwrap();
    assert_eq!(local.release_id, "rel-2");
    assert_eq!(kept.kept_release().unwrap().release_id, "rel-2");

    // At the next launch, the files only the old release played go.
    kept.remove_unused();
    let now = files_in(&kept);
    assert_eq!(now.len(), newer_links.len());
    assert!(old_files.iter().all(|name| !now.contains(name)));
    assert!(kept.kept_release().is_some());
}

#[test]
fn remove_unused_deletes_only_what_the_kept_release_does_not_play() {
    let server = Server::start();
    let app = app_dir("unused");
    let kept = Kept::new(&app, &server.url(CANVAS));
    let fetched = released(&server, "s1", "fingerprint-1", 1);
    let files = kept.folder().join("files");

    // Without a kept release, nothing goes.
    fs::create_dir_all(&files).unwrap();
    fs::write(files.join("stray.mp4"), b"x").unwrap();
    kept.remove_unused();
    assert!(files.join("stray.mp4").exists());

    kept.keep(&fetched).unwrap();
    fs::write(files.join("0123456789abcdef.part"), b"half").unwrap();
    let used = files_in(&kept).len() - 2;
    kept.remove_unused();
    assert!(!files.join("stray.mp4").exists());
    assert!(!files.join("0123456789abcdef.part").exists());
    assert_eq!(files_in(&kept).len(), used);
    assert!(kept.kept_release().is_some());
}

#[test]
fn fetch_file_gives_a_kept_file_downloading_it_once() {
    let server = Server::start();
    let app = app_dir("fetch-file");
    let kept = Kept::new(&app, &server.url(CANVAS));
    let fetched = released(&server, "s1", "fingerprint-1", 1);
    let all = links(&fetched.delivery_json).unwrap();

    // Several threads, the same links and different ones.
    std::thread::scope(|scope| {
        for t in 0..8 {
            let (kept, all) = (&kept, &all);
            scope.spawn(move || {
                for link in all.iter().skip(t % 3) {
                    let file = kept.fetch_file(link).unwrap();
                    assert_eq!(file, kept.file_for(link));
                    assert!(file.is_file());
                }
            });
        }
    });
    for link in &all {
        let path = link.split("127.0.0.1").nth(1).unwrap();
        let path = path[path.find('/').unwrap()..].split('?').next().unwrap();
        let name = path.rsplit('/').next().unwrap();
        assert_eq!(fs::read(kept.file_for(link)).unwrap(), file_body(name));
        // A link asked by several threads at once is downloaded once.
        assert_eq!(server.asked(path), 1, "{path}");
    }
    assert!(files_in(&kept).iter().all(|name| !name.ends_with(".part")));
    assert_eq!(files_in(&kept).len(), all.len());

    // Kept already: `keep` downloads nothing more, `fetch_file` neither.
    let asked = server.requests().len();
    kept.fetch_file(&all[0]).unwrap();
    kept.keep(&fetched).unwrap();
    assert_eq!(server.requests().len(), asked);

    // A local path from the kept release is its own file.
    let local = kept.kept_release().unwrap();
    let path = links(&local.delivery_json).unwrap().remove(0);
    assert_eq!(kept.fetch_file(&path).unwrap(), Path::new(&path));
    fs::remove_file(&path).unwrap();
    assert!(matches!(kept.fetch_file(&path), Err(KeepError::Io(_))));

    // A link that cannot be downloaded leaves nothing.
    let missing = server.url("/files/missing.mp4?sig=s1");
    assert!(matches!(
        kept.fetch_file(&missing),
        Err(KeepError::Network(_))
    ));
    assert!(!kept.file_for(&missing).exists());
}

#[test]
fn the_install_id_is_made_once_and_kept() {
    let app = app_dir("install-id");
    let id = Kept::install_id(&app);
    assert_eq!(id.len(), 36);
    assert_eq!(id, id.to_lowercase());
    let parts: Vec<&str> = id.split('-').collect();
    assert_eq!(
        parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
        [8, 4, 4, 4, 12]
    );
    assert!(parts[2].starts_with('4'), "version 4: {id}");
    assert!(
        matches!(parts[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
        "variant: {id}"
    );
    let file = app.join("MaskoPlayer").join("install-id");
    assert_eq!(fs::read_to_string(&file).unwrap(), id);
    assert_eq!(Kept::install_id(&app), id);

    // Another app has its own.
    assert_ne!(Kept::install_id(&app_dir("install-id-other")), id);

    // Something that is not an id is replaced.
    fs::write(&file, "not an id").unwrap();
    let fresh = Kept::install_id(&app);
    assert_ne!(fresh, id);
    assert_eq!(fs::read_to_string(&file).unwrap(), fresh);
    // One written by hand in capitals is read as it is, as on the Mac.
    fs::write(&file, "0B8C3C39-5F4E-4D36-9A8E-3D9F6F3F2A10").unwrap();
    assert_eq!(
        Kept::install_id(&app),
        "0B8C3C39-5F4E-4D36-9A8E-3D9F6F3F2A10"
    );
}
