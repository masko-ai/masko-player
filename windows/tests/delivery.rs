//! Asking the canvas URL, and reading a delivery for the Windows player
//! (src/delivery.rs), against a local server.

#[path = "delivery_server.rs"]
mod server;

use masko_player_windows::delivery::{
    edge_media, fetch, graph_info, links, pose_stills, request_url, same_release,
    signed_links_expire_at, FetchError, GraphInfo, Sound, TARGET,
};
use serde_json::json;
use server::{delivery, Reply, Server, CANVAS};

const KEY: &str = "masko_app_test_secret";
const INSTALL: &str = "0b8c3c39-5f4e-4d36-9a8e-3d9f6f3f2a10";

#[test]
fn the_request_adds_target_and_formats_unless_the_url_names_them() {
    let url = "https://api.masko.ai/v1/canvases/c1/delivery";
    assert_eq!(TARGET, "windows");
    assert_eq!(
        request_url(url, TARGET),
        format!("{url}?target=windows&formats=av1")
    );
    assert_eq!(
        request_url(url, "desktop"),
        format!("{url}?target=desktop&formats=av1")
    );
    assert_eq!(
        request_url(&format!("{url}?target=web"), TARGET),
        format!("{url}?target=web&formats=av1")
    );
    assert_eq!(
        request_url(&format!("{url}?formats=av1,hevc&size=256"), TARGET),
        format!("{url}?formats=av1,hevc&size=256&target=windows")
    );
    assert_eq!(
        request_url(&format!("{url}?target=apple&formats=av1"), TARGET),
        format!("{url}?target=apple&formats=av1")
    );
    assert_eq!(
        request_url(&format!("{url}?"), TARGET),
        format!("{url}?target=windows&formats=av1")
    );
    assert_eq!(
        request_url(&format!("{url}#top"), TARGET),
        format!("{url}?target=windows&formats=av1#top")
    );
}

#[test]
fn fetch_asks_with_the_app_key_and_install_id_and_gives_the_latest_release() {
    let server = Server::start();
    let d = delivery(&server.url(""), "s1", "fingerprint-1");
    server.route(CANVAS, Reply::ok(server::answer_body("rel-7", 7, &d)));

    let fetched = fetch(&server.url(CANVAS), KEY, INSTALL, TARGET).unwrap();

    assert_eq!(fetched.release_id, "rel-7");
    assert_eq!(fetched.number, 7);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&fetched.delivery_json).unwrap(),
        d
    );
    // The engine accepts it as it came.
    assert!(masko_player_engine::load(&fetched.delivery_json).is_ok());
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].target,
        format!("{CANVAS}?target=windows&formats=av1")
    );
    assert_eq!(
        requests[0].headers["authorization"],
        format!("Bearer {KEY}")
    );
    assert_eq!(requests[0].headers["masko-install-id"], INSTALL);
}

#[test]
fn fetch_can_ask_for_another_target() {
    let server = Server::start();
    let d = delivery(&server.url(""), "s1", "fingerprint-1");
    server.route(CANVAS, Reply::ok(server::answer_body("rel-1", 1, &d)));
    fetch(
        &server.url(&format!("{CANVAS}?size=512")),
        KEY,
        INSTALL,
        "desktop",
    )
    .unwrap();
    assert_eq!(
        server.requests()[0].target,
        format!("{CANVAS}?size=512&target=desktop&formats=av1")
    );
}

#[test]
fn an_error_answer_gives_its_status_and_message_never_the_key() {
    let server = Server::start();
    server.route(
        CANVAS,
        Reply::status(
            401,
            r#"{"error":{"code":"unauthorized","message":"Invalid app key"}}"#,
        ),
    );
    let error = fetch(&server.url(CANVAS), KEY, INSTALL, TARGET).unwrap_err();
    assert!(
        matches!(&error, FetchError::Status(401, m) if m == "Invalid app key"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the canvas URL answered 401: Invalid app key"
    );
    assert!(!format!("{error} {error:?}").contains(KEY));

    server.route(CANVAS, Reply::status(503, "busy"));
    let error = fetch(&server.url(CANVAS), KEY, INSTALL, TARGET).unwrap_err();
    assert!(
        matches!(&error, FetchError::Status(503, m) if m.is_empty()),
        "{error:?}"
    );
    assert_eq!(error.to_string(), "the canvas URL answered 503");
}

#[test]
fn an_answer_without_a_delivery_is_unreadable() {
    let server = Server::start();
    for body in [
        r#"{"data":{"release_id":"r","number":1}}"#,
        "<html>",
        r#"{"data":{"delivery":"x"}}"#,
    ] {
        server.route(CANVAS, Reply::ok(body));
        let error = fetch(&server.url(CANVAS), KEY, INSTALL, TARGET).unwrap_err();
        assert!(
            matches!(error, FetchError::Unreadable(_)),
            "{body}: {error:?}"
        );
    }
}

#[test]
fn a_server_that_cannot_be_reached_is_a_network_error() {
    let server = Server::start();
    let url = server.url(CANVAS);
    server.stop();
    let error = fetch(&url, KEY, INSTALL, TARGET).unwrap_err();
    assert!(matches!(error, FetchError::Network(_)), "{error:?}");
    assert!(!format!("{error} {error:?}").contains(KEY));
}

fn sample() -> (String, String) {
    let base = "https://cdn.example.invalid";
    let d = delivery(base, "s1", "fingerprint-1");
    (base.to_owned(), d.to_string())
}

#[test]
fn edge_media_reads_the_av1_files_sounds_and_movement() {
    let (base, d) = sample();
    let media = edge_media(&d).unwrap();
    assert_eq!(media.len(), 12);

    let idle = &media["loop-idle"];
    assert_eq!(idle.id, "loop-idle");
    assert_eq!(idle.source.as_deref(), Some("idle"));
    assert_eq!(idle.target.as_deref(), Some("idle"));
    assert!(idle.is_loop);
    assert_eq!(
        idle.video.as_deref(),
        Some(format!("{base}/files/loop-idle.mp4?sig=s1").as_str())
    );
    assert!(idle.variants.is_empty());
    assert_eq!(idle.duration_s, 4.0);
    assert!(!idle.has_audio);
    assert_eq!(idle.sound, None);
    assert_eq!(idle.movement, None);

    let hop = &media["idle-to-alert"];
    assert!(!hop.is_loop);
    assert_eq!(hop.target.as_deref(), Some("alert"));
    assert_eq!(hop.duration_s, 1.0);
    assert_eq!(
        hop.sound,
        Some(Sound {
            url: format!("{base}/files/alert.m4a?sig=s1"),
            volume: 0.5,
            looping: false
        })
    );
    assert_eq!(hop.movement, Some(json!({ "kind": "hop", "x": 120 })));

    let alert = &media["loop-alert"];
    assert!(alert.has_audio);
    // A volume above 1 plays at 1.
    assert_eq!(
        alert.sound.as_ref().map(|s| (s.volume, s.looping)),
        Some((1.0, true))
    );

    // HEVC and WebM never play on Windows.
    assert_eq!(media["watching-to-idle"].video, None);
    assert!(media["watching-to-idle"].variants.is_empty());
    assert_eq!(media["watching-to-idle"].file_for_pixels(Some(100.0)), None);

    let watching = &media["loop-watching"];
    assert_eq!(
        watching.variants,
        vec![
            (256, format!("{base}/files/loop-watching-256.mp4?sig=s1")),
            (512, format!("{base}/files/loop-watching-512.mp4?sig=s1")),
        ]
    );
}

#[test]
fn the_file_for_a_size_is_the_smallest_large_enough_else_the_base() {
    let (base, d) = sample();
    let media = edge_media(&d).unwrap();
    let watching = &media["loop-watching"];
    let file = |name: &str| format!("{base}/files/{name}?sig=s1");
    assert_eq!(
        watching.file_for_pixels(Some(200.0)),
        Some((file("loop-watching-256.mp4"), "loop-watching@256".into()))
    );
    assert_eq!(
        watching.file_for_pixels(Some(256.0)),
        Some((file("loop-watching-256.mp4"), "loop-watching@256".into()))
    );
    assert_eq!(
        watching.file_for_pixels(Some(300.0)),
        Some((file("loop-watching-512.mp4"), "loop-watching@512".into()))
    );
    assert_eq!(
        watching.file_for_pixels(Some(900.0)),
        Some((file("loop-watching.mp4"), "loop-watching".into()))
    );
    assert_eq!(
        watching.file_for_pixels(None),
        Some((file("loop-watching.mp4"), "loop-watching".into()))
    );

    // Sizes only: the largest when none is large enough.
    let sizes_only = json!({
        "runtimeGraph": { "nodes": [], "edges": [{ "id": "e", "videos": { "variants": {
            "128": { "av1": "https://x.invalid/e-128.mp4" },
            "64": { "av1": "https://x.invalid/e-64.mp4", "hevc": "https://x.invalid/e-64.mov" },
            "32": { "hevc": "https://x.invalid/e-32.mov" },
            "big": { "av1": "https://x.invalid/e-big.mp4" }
        } } }] }
    });
    let e = &edge_media(&sizes_only.to_string()).unwrap()["e"];
    assert_eq!(e.video, None);
    assert_eq!(e.variants.len(), 2);
    assert_eq!(
        e.file_for_pixels(Some(1000.0)),
        Some(("https://x.invalid/e-128.mp4".into(), "e@128".into()))
    );
    assert_eq!(
        e.file_for_pixels(Some(10.0)),
        Some(("https://x.invalid/e-64.mp4".into(), "e@64".into()))
    );
}

#[test]
fn pose_stills_and_graph_info() {
    let (base, d) = sample();
    let stills = pose_stills(&d).unwrap();
    assert_eq!(stills.len(), 3);
    assert_eq!(stills["idle"], format!("{base}/files/idle.png?sig=s1"));
    assert!(!stills.contains_key("celebrating"));

    assert_eq!(
        graph_info(&d).unwrap(),
        GraphInfo {
            initial_node: Some("idle".into()),
            auto_play: true,
            capabilities: None
        }
    );
    let able = json!({ "runtimeGraph": { "nodes": [], "edges": [],
        "capabilities": { "actions": { "nap": { "input": "action::nap", "name": "Nap" } } } } });
    assert_eq!(
        graph_info(&able.to_string()).unwrap().capabilities,
        Some(json!({ "actions": { "nap": { "input": "action::nap", "name": "Nap" } } })),
        "the graph's capabilities, unchanged"
    );
    let paused = json!({ "runtimeGraph": { "initialNode": "a", "autoPlay": false, "nodes": [], "edges": [] } });
    assert!(!graph_info(&paused.to_string()).unwrap().auto_play);
    let unset = json!({ "runtimeGraph": { "nodes": [], "edges": [] } });
    assert_eq!(
        graph_info(&unset.to_string()).unwrap(),
        GraphInfo {
            initial_node: None,
            auto_play: true,
            capabilities: None
        }
    );
}

#[test]
fn links_are_the_av1_clips_stills_and_sounds_once_each() {
    let (base, d) = sample();
    let links = links(&d).unwrap();
    // 11 clips with AV1, 2 sizes, 1 sound shared by two edges, 3 stills.
    assert_eq!(links.len(), 11 + 2 + 1 + 3, "{links:#?}");
    assert!(links
        .iter()
        .all(|l| l.starts_with(&base) && l.ends_with("?sig=s1")));
    assert!(links
        .iter()
        .all(|l| !l.contains(".mov") && !l.contains(".webm")));
    assert!(links.contains(&format!("{base}/files/alert.m4a?sig=s1")));
    assert!(links.contains(&format!("{base}/files/loop-watching-512.mp4?sig=s1")));
    assert!(links.contains(&format!("{base}/files/watching.png?sig=s1")));
    assert!(!links.contains(&format!("{base}/files/watching-to-idle.mp4?sig=s1")));
}

#[test]
fn unreadable_deliveries_are_refused() {
    for text in [
        "not json",
        "{}",
        r#"{"runtimeGraph":{"nodes":[]}}"#,
        r#"{"runtimeGraph":{"nodes":[{}],"edges":[]}}"#,
    ] {
        assert!(
            matches!(edge_media(text), Err(FetchError::Unreadable(_))),
            "{text}"
        );
        assert!(
            matches!(links(text), Err(FetchError::Unreadable(_))),
            "{text}"
        );
        assert!(
            matches!(pose_stills(text), Err(FetchError::Unreadable(_))),
            "{text}"
        );
        assert!(
            matches!(graph_info(text), Err(FetchError::Unreadable(_))),
            "{text}"
        );
    }
    // A media field of the wrong type is ignored, as the engine ignores it.
    let odd = json!({ "runtimeGraph": { "nodes": [{ "id": "a", "transparentThumbnailUrl": 3 }],
        "edges": [{ "id": "e", "videos": { "av1": false }, "sound": { "volume": 0.2 }, "duration": "4" }] } });
    let media = edge_media(&odd.to_string()).unwrap();
    assert_eq!(media["e"].video, None);
    assert_eq!(media["e"].sound, None);
    assert_eq!(media["e"].duration_s, 0.0);
    assert!(pose_stills(&odd.to_string()).unwrap().is_empty());
}

#[test]
fn signed_links_expire_at_their_time_in_unix_milliseconds() {
    let (_, d) = sample();
    assert_eq!(signed_links_expire_at(&d), Some(1_793_880_000_000));

    let with = |access: serde_json::Value| {
        json!({ "deliveryAccess": access, "runtimeGraph": {} }).to_string()
    };
    assert_eq!(
        signed_links_expire_at(&with(
            json!({ "urlMode": "signed", "signedUrlExpiresAt": "2026-11-05T12:00:00.25+02:00" })
        )),
        Some(1_793_872_800_250)
    );
    assert_eq!(
        signed_links_expire_at(&with(
            json!({ "urlMode": "signed", "signedUrlExpiresAt": "2026-11-05T12:00:00Z" })
        )),
        Some(1_793_880_000_000)
    );
    assert_eq!(
        signed_links_expire_at(&with(
            json!({ "urlMode": "public", "signedUrlExpiresAt": "2026-11-05T12:00:00Z" })
        )),
        None
    );
    assert_eq!(
        signed_links_expire_at(&with(json!({ "urlMode": "signed" }))),
        None
    );
    assert_eq!(
        signed_links_expire_at(&with(
            json!({ "urlMode": "signed", "signedUrlExpiresAt": "tomorrow" })
        )),
        None
    );
    assert_eq!(signed_links_expire_at("{}"), None);
}

#[test]
fn the_same_release_differs_only_in_its_links() {
    let a = delivery("https://cdn.example.invalid", "s1", "fingerprint-1");
    let fresh = delivery("https://other.example.invalid", "s2", "fingerprint-1");
    assert!(same_release(&a.to_string(), &fresh.to_string()));

    // Another release id is another release.
    let other = delivery("https://cdn.example.invalid", "s1", "fingerprint-2");
    assert!(!same_release(&a.to_string(), &other.to_string()));

    // The same id with another graph is too.
    let mut longer = a.clone();
    longer["runtimeGraph"]["edges"][0]["duration"] = json!(5);
    assert!(!same_release(&a.to_string(), &longer.to_string()));

    // 4 and 4.0 are the same number; key order does not matter.
    let mut float = a.clone();
    float["runtimeGraph"]["edges"][0]["duration"] = json!(4.0);
    assert!(same_release(&a.to_string(), &float.to_string()));

    // Host data outside the graph and the release id is not compared.
    let mut later = a.clone();
    later["deliveryAccess"]["lastFetchedAt"] = json!("2026-11-06T00:00:00.000Z");
    assert!(same_release(&a.to_string(), &later.to_string()));

    assert!(!same_release(&a.to_string(), "not json"));
    assert!(!same_release("{}", "{}"));
}
