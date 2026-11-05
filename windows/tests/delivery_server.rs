//! A tiny HTTP server on 127.0.0.1 standing in for the canvas URL and its
//! file links, and a realistic delivery to serve, for `tests/delivery.rs`
//! and `tests/kept.rs` (each includes this file). Nothing here reaches a
//! real Masko server.
#![allow(dead_code)]

use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// What the server answers for a path.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// Announce the whole body but send half of it, then close.
    pub cut: bool,
}

impl Reply {
    pub fn ok(body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status: 200,
            body: body.into(),
            cut: false,
        }
    }

    pub fn status(status: u16, body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            status,
            body: body.into(),
            cut: false,
        }
    }

    pub fn cut(body: impl Into<Vec<u8>>) -> Reply {
        Reply {
            cut: true,
            ..Reply::ok(body)
        }
    }
}

/// A request the server read: its target (path and query) and headers, names
/// lowercase.
#[derive(Clone, Debug)]
pub struct Request {
    pub target: String,
    pub headers: HashMap<String, String>,
}

pub struct Server {
    pub port: u16,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let requests: Arc<Mutex<Vec<Request>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (routes, requests, stop) = (routes.clone(), requests.clone(), stop.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (routes, requests) = (routes.clone(), requests.clone());
                    std::thread::spawn(move || answer(stream, &routes, &requests));
                }
            })
        };
        Server {
            port,
            routes,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// Answers `reply` for `path` (without its query).
    pub fn route(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// How many requests asked for `path` (without its query).
    pub fn asked(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.target.split('?').next() == Some(path))
            .count()
    }

    /// Stops answering: the port refuses connections afterwards.
    pub fn stop(mut self) {
        self.shut();
    }

    fn shut(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shut();
    }
}

fn answer(
    mut stream: TcpStream,
    routes: &Mutex<HashMap<String, Reply>>,
    requests: &Mutex<Vec<Request>>,
) {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(1) => head.push(byte[0]),
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let mut lines = head.split("\r\n");
    let target = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let path = target.split('?').next().unwrap_or("/").to_owned();
    requests.lock().unwrap().push(Request { target, headers });
    let reply = routes
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or_else(|| Reply::status(404, r#"{"error":{"message":"Not found"}}"#));
    let sent = if reply.cut {
        &reply.body[..reply.body.len() / 2]
    } else {
        &reply.body[..]
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.status,
        reply.body.len()
    );
    let _ = stream.write_all(sent);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

/// The canvas URL path the tests ask.
pub const CANVAS: &str = "/v1/canvases/c1/delivery";

/// A release delivery as the canvas URL gives one to an app asking with
/// `formats=av1` (manifest mode, signed links), built from the Otto canvas of
/// `spec/fixtures`. Its files are `<base>/files/<name>?sig=<sig>`:
///
/// - every clip has an AV1 file (with HEVC and WebM beside it, never read),
///   but `watching-to-idle`, which has HEVC and WebM only;
/// - `loop-watching` also has AV1 sizes 256 and 512;
/// - `idle-to-alert` has a sound and a movement, `loop-alert` a looping
///   sound and its own voice; both sounds are the same file;
/// - every pose but `celebrating` has a still.
pub fn delivery(base: &str, sig: &str, release_id: &str) -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures/canvases/otto.json");
    let mut delivery: Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let link = |name: &str| format!("{base}/files/{name}?sig={sig}");
    let graph = delivery["runtimeGraph"].as_object_mut().unwrap();
    for node in graph["nodes"].as_array_mut().unwrap() {
        let id = node["id"].as_str().unwrap().to_owned();
        if id != "celebrating" {
            node["transparentThumbnailUrl"] = json!(link(&format!("{id}.png")));
        }
    }
    for edge in graph["edges"].as_array_mut().unwrap() {
        let id = edge["id"].as_str().unwrap().to_owned();
        let mut videos = json!({
            "hevc": link(&format!("{id}.mov")),
            "webm": link(&format!("{id}.webm")),
        });
        if id != "watching-to-idle" {
            videos["av1"] = json!(link(&format!("{id}.mp4")));
        }
        if id == "loop-watching" {
            videos["variants"] = json!({
                "512": { "av1": link("loop-watching-512.mp4"), "hevc": link("loop-watching-512.mov") },
                "256": { "av1": link("loop-watching-256.mp4") },
            });
        }
        edge["videos"] = videos;
        if id == "idle-to-alert" {
            edge["sound"] = json!({ "url": link("alert.m4a"), "volume": 0.5 });
            edge["movement"] = json!({ "kind": "hop", "x": 120 });
        }
        if id == "loop-alert" {
            edge["sound"] = json!({ "url": link("alert.m4a"), "volume": 3, "loop": true });
            edge["hasAudio"] = json!(true);
        }
    }
    delivery["target"] = json!("windows");
    delivery["mode"] = json!("manifest");
    delivery["release"] = json!({ "id": release_id, "mascotVersion": 4 });
    delivery["deliveryAccess"] = json!({
        "urlMode": "signed",
        "signedUrlExpiresAt": "2026-11-05T12:00:00.000Z",
        "lastFetchedAt": "2026-11-05T11:00:00.000Z",
    });
    delivery
}

/// The canvas URL's answer around a delivery.
pub fn answer_body(release_id: &str, number: u64, delivery: &Value) -> String {
    json!({ "data": { "release_id": release_id, "number": number, "delivery": delivery } })
        .to_string()
}

/// The bytes the server gives for a file.
pub fn file_body(name: &str) -> Vec<u8> {
    format!("bytes of {name} ").repeat(40).into_bytes()
}

/// Serves `delivery` at the canvas URL and every file it links.
pub fn serve(server: &Server, release_id: &str, number: u64, delivery: &Value) {
    server.route(CANVAS, Reply::ok(answer_body(release_id, number, delivery)));
    let mut links = Vec::new();
    collect_links(delivery, &mut links);
    for link in links {
        let path = link.split("127.0.0.1").nth(1).unwrap();
        let path = &path[path.find('/').unwrap()..];
        let path = path.split('?').next().unwrap();
        let name = path.rsplit('/').next().unwrap();
        server.route(path, Reply::ok(file_body(name)));
    }
}

fn collect_links(value: &Value, links: &mut Vec<String>) {
    match value {
        Value::String(s) if s.starts_with("http://127.0.0.1") => links.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| collect_links(v, links)),
        Value::Object(fields) => fields.values().for_each(|v| collect_links(v, links)),
        _ => {}
    }
}

/// A new empty folder for one test's app data.
pub fn app_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "masko-player-windows-tests-{}-{test}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
