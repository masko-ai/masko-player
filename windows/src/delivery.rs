//! A canvas read from its URL with an app key (spec/player.md, "A canvas from
//! its URL"), and what the Windows player reads from a release delivery for
//! itself: its AV1 clips, stills, sounds, movement, the poses each edge joins
//! and when signed links expire. The engine reads the rules from the same
//! JSON.
//!
//! The Windows player plays stacked AV1 files only (`videos.av1`, at its base
//! and in every size of `videos.variants`), so a clip without one has no file
//! here; `webm`, `hevc` and `stacked` are never read nor kept.

use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::OnceLock;
use std::time::Duration;

/// The `target` the Windows player asks for. Until the server knows it, a
/// player or a test may ask for `desktop` or `web` instead (`fetch` takes
/// the target).
pub const TARGET: &str = "windows";
/// The `formats` the Windows player asks for: it plays only stacked AV1.
pub const FORMATS: &str = "av1";

/// How long asking the canvas URL may take, as on the Mac.
const ASK_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest delivery read. A delivery is a few hundred kilobytes at most.
const DELIVERY_LIMIT: u64 = 32 * 1024 * 1024;

/// The canvas URL's answer: its latest release.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// The release's id on the server (`data.release_id`).
    pub release_id: String,
    /// The release's number on its canvas (`data.number`); newer is larger.
    pub number: u64,
    /// The `data.delivery` object, as JSON text.
    pub delivery_json: String,
}

/// Why the canvas URL gave no release, or why a delivery cannot be read.
#[derive(Debug)]
pub enum FetchError {
    /// The URL could not be reached, or its answer broke off.
    Network(String),
    /// The URL answered another status than 200, with the server's message.
    Status(u16, String),
    /// The answer, or a delivery, is not one the player can read.
    Unreadable(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Network(detail) => write!(f, "the canvas URL cannot be reached: {detail}"),
            FetchError::Status(status, message) if message.is_empty() => {
                write!(f, "the canvas URL answered {status}")
            }
            FetchError::Status(status, message) => {
                write!(f, "the canvas URL answered {status}: {message}")
            }
            FetchError::Unreadable(detail) => write!(f, "unreadable delivery: {detail}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// The HTTP client every request shares: answers of every status are read
/// (the canvas URL's errors carry a message), a stalled server gives up, and
/// the app key never follows a redirect to another host.
pub(crate) fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            // A clip is a few megabytes; ten minutes is a very slow line.
            .timeout_recv_body(Some(Duration::from_secs(600)))
            .user_agent(concat!("masko-player-windows/", env!("CARGO_PKG_VERSION")))
            .build()
            .new_agent()
    })
}

/// The canvas URL with `target` and `formats=av1`, unless it names them.
/// `target` is a plain word (`windows`, `desktop`, `web`).
pub fn request_url(canvas_url: &str, target: &str) -> String {
    let (main, fragment) = match canvas_url.find('#') {
        Some(at) => canvas_url.split_at(at),
        None => (canvas_url, ""),
    };
    let (base, query) = match main.split_once('?') {
        Some((base, query)) => (base, query),
        None => (main, ""),
    };
    let mut items: Vec<String> = query
        .split('&')
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect();
    let names = |items: &[String], name: &str| {
        items
            .iter()
            .any(|item| item.split('=').next() == Some(name))
    };
    if !names(&items, "target") {
        items.push(format!("target={target}"));
    }
    if !names(&items, "formats") {
        items.push(format!("formats={FORMATS}"));
    }
    format!("{base}?{}{fragment}", items.join("&"))
}

/// Asks the canvas URL for its latest release, with the app key and this
/// installed copy's id (blocking, at most 30 seconds).
pub fn fetch(
    canvas_url: &str,
    app_key: &str,
    install_id: &str,
    target: &str,
) -> Result<Fetched, FetchError> {
    let url = request_url(canvas_url, target);
    let mut response = agent()
        .get(&url)
        .header("Authorization", format!("Bearer {app_key}"))
        .header("Masko-Install-Id", install_id)
        .config()
        .timeout_global(Some(ASK_TIMEOUT))
        .build()
        .call()
        .map_err(|e| FetchError::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(DELIVERY_LIMIT)
        .read_to_string();
    let answer = body
        .as_deref()
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    if status != 200 {
        let message = answer
            .as_ref()
            .and_then(|a| a.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Err(FetchError::Status(status, message.to_owned()));
    }
    let body = body.map_err(|e| FetchError::Network(e.to_string()))?;
    let answer = answer.ok_or_else(|| {
        FetchError::Unreadable(format!(
            "the canvas URL gave no JSON ({} bytes)",
            body.len()
        ))
    })?;
    let data = answer.get("data");
    let delivery = data
        .and_then(|d| d.get("delivery"))
        .filter(|d| d.is_object())
        .ok_or_else(|| FetchError::Unreadable("the canvas URL gave no delivery".into()))?;
    let release_id = data
        .and_then(|d| d.get("release_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let number = data
        .and_then(|d| d.get("number"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(Fetched {
        release_id,
        number,
        delivery_json: delivery.to_string(),
    })
}

/// An edge's sound (rules.md 9.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub url: String,
    /// Between 0 and 1; 1 when the delivery gives none.
    pub volume: f32,
    pub looping: bool,
}

/// What the Windows player plays for an edge.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeMedia {
    /// The edge id, which names a file choice (`file_for_pixels`).
    pub id: String,
    /// The pose it leaves (`*` for any pose) and the pose it reaches.
    pub source: Option<String>,
    pub target: Option<String>,
    /// A pose's own clip, which loops on its pose.
    pub is_loop: bool,
    /// The stacked AV1 file, None when the clip has none (it cannot play on
    /// Windows).
    pub video: Option<String>,
    /// The AV1 file of each size of `videos.variants` (manifest mode),
    /// smallest first.
    pub variants: Vec<(u32, String)>,
    /// The clip's length in seconds at rate 1.
    pub duration_s: f64,
    /// The file has its own voice.
    pub has_audio: bool,
    pub sound: Option<Sound>,
    /// Host data for the app, unchanged.
    pub movement: Option<Value>,
}

impl EdgeMedia {
    /// The file to play on a mascot `pixels` wide on screen (its size times
    /// the screen density): the smallest size at least that large, else the
    /// base file, else the largest size (player.md, "The file and the size").
    /// The key names the choice (`edge@size`, or `edge` for the base file),
    /// so a decoded clip outlives a link refresh.
    pub fn file_for_pixels(&self, pixels: Option<f64>) -> Option<(String, String)> {
        if let Some(pixels) = pixels {
            if let Some((size, file)) = self
                .variants
                .iter()
                .find(|(size, _)| *size as f64 >= pixels)
            {
                return Some((file.clone(), format!("{}@{size}", self.id)));
            }
        }
        if let Some(file) = &self.video {
            return Some((file.clone(), self.id.clone()));
        }
        self.variants
            .last()
            .map(|(size, file)| (file.clone(), format!("{}@{size}", self.id)))
    }
}

/// The graph fields the player reads before it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphInfo {
    /// The pose the mascot starts on.
    pub initial_node: Option<String>,
    /// The graph's `autoPlay`, true when it gives none (rules.md 11.1).
    pub auto_play: bool,
}

fn parse(delivery_json: &str) -> Result<Value, FetchError> {
    serde_json::from_str(delivery_json).map_err(|e| FetchError::Unreadable(e.to_string()))
}

/// A delivery's graph, its nodes and its edges.
struct Graph<'a> {
    fields: &'a Map<String, Value>,
    nodes: &'a [Value],
    edges: &'a [Value],
}

/// The graph, its nodes and its edges. Only these and the `id` of each node
/// and edge are required; a media field of the wrong type is ignored rather
/// than refusing a delivery the engine accepts, as on the Mac.
fn graph(root: &Value) -> Result<Graph<'_>, FetchError> {
    let graph = root
        .get("runtimeGraph")
        .and_then(Value::as_object)
        .ok_or_else(|| FetchError::Unreadable("no runtimeGraph".into()))?;
    let list = |name: &str| {
        graph
            .get(name)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| FetchError::Unreadable(format!("runtimeGraph has no {name}")))
    };
    let (nodes, edges) = (list("nodes")?, list("edges")?);
    for (kind, items) in [("a node", nodes), ("an edge", edges)] {
        if items
            .iter()
            .any(|item| item.get("id").and_then(Value::as_str).is_none())
        {
            return Err(FetchError::Unreadable(format!("{kind} has no id")));
        }
    }
    Ok(Graph {
        fields: graph,
        nodes,
        edges,
    })
}

fn text<'a>(object: &'a Value, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn edge(e: &Value) -> EdgeMedia {
    let videos = e.get("videos");
    let mut variants: Vec<(u32, String)> = videos
        .and_then(|v| v.get("variants"))
        .and_then(Value::as_object)
        .map(|sizes| {
            sizes
                .iter()
                .filter_map(|(size, files)| {
                    Some((size.parse().ok()?, text(files, "av1")?.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default();
    variants.sort_by_key(|(size, _)| *size);
    let sound = e.get("sound").and_then(|s| {
        Some(Sound {
            url: text(s, "url")?.to_owned(),
            volume: s
                .get("volume")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0) as f32,
            looping: s.get("loop").and_then(Value::as_bool).unwrap_or(false),
        })
    });
    EdgeMedia {
        id: text(e, "id").unwrap_or_default().to_owned(),
        source: text(e, "source").map(str::to_owned),
        target: text(e, "target").map(str::to_owned),
        is_loop: e.get("isLoop").and_then(Value::as_bool).unwrap_or(false),
        video: videos.and_then(|v| text(v, "av1")).map(str::to_owned),
        variants,
        duration_s: e.get("duration").and_then(Value::as_f64).unwrap_or(0.0),
        has_audio: e.get("hasAudio").and_then(Value::as_bool).unwrap_or(false),
        sound,
        movement: e.get("movement").filter(|m| !m.is_null()).cloned(),
    }
}

/// Every edge's media, by edge id.
pub fn edge_media(delivery_json: &str) -> Result<HashMap<String, EdgeMedia>, FetchError> {
    let root = parse(delivery_json)?;
    Ok(graph(&root)?
        .edges
        .iter()
        .map(edge)
        .map(|media| (media.id.clone(), media))
        .collect())
}

/// Each pose's still (`transparentThumbnailUrl`), by pose id; a pose without
/// one shows nothing in place of video.
pub fn pose_stills(delivery_json: &str) -> Result<HashMap<String, String>, FetchError> {
    let root = parse(delivery_json)?;
    Ok(graph(&root)?
        .nodes
        .iter()
        .filter_map(|n| {
            Some((
                text(n, "id")?.to_owned(),
                text(n, "transparentThumbnailUrl")?.to_owned(),
            ))
        })
        .collect())
}

/// The pose the mascot starts on and the graph's `autoPlay`.
pub fn graph_info(delivery_json: &str) -> Result<GraphInfo, FetchError> {
    let root = parse(delivery_json)?;
    let graph = graph(&root)?.fields;
    Ok(GraphInfo {
        initial_node: graph
            .get("initialNode")
            .and_then(Value::as_str)
            .map(str::to_owned),
        auto_play: graph
            .get("autoPlay")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

/// Every file link the Windows player plays: AV1 clips, stills and sounds,
/// deduplicated, in canvas order.
pub fn links(delivery_json: &str) -> Result<Vec<String>, FetchError> {
    links_in(&parse(delivery_json)?)
}

pub(crate) fn links_in(root: &Value) -> Result<Vec<String>, FetchError> {
    let Graph { nodes, edges, .. } = graph(root)?;
    let mut seen = HashSet::new();
    let mut links = Vec::new();
    let mut add = |link: &str| {
        if seen.insert(link.to_owned()) {
            links.push(link.to_owned());
        }
    };
    for media in edges.iter().map(edge) {
        media.video.iter().for_each(|l| add(l));
        media.variants.iter().for_each(|(_, l)| add(l));
        media.sound.iter().for_each(|s| add(&s.url));
    }
    nodes
        .iter()
        .filter_map(|n| text(n, "transparentThumbnailUrl"))
        .for_each(&mut add);
    Ok(links)
}

/// `deliveryAccess.signedUrlExpiresAt` as unix milliseconds, for signed
/// links only (`urlMode: "signed"`).
pub fn signed_links_expire_at(delivery_json: &str) -> Option<u64> {
    let root: Value = serde_json::from_str(delivery_json).ok()?;
    let access = root.get("deliveryAccess")?;
    if text(access, "urlMode") != Some("signed") {
        return None;
    }
    unix_ms(text(access, "signedUrlExpiresAt")?)
}

/// Whether two deliveries are the same release, so that only their links
/// may differ: the same `release.id` and the same graph once every link is
/// left out (player.md, `updateDelivery`). A link is a `http(s)`, `blob`,
/// `data` or `file` URL, or a file the player keeps (`Kept`), so a kept
/// release is the same release as the delivery it was kept from.
pub fn same_release(a_json: &str, b_json: &str) -> bool {
    let (Ok(a), Ok(b)) = (parse(a_json), parse(b_json)) else {
        return false;
    };
    let (Some(ga), Some(gb)) = (a.get("runtimeGraph"), b.get("runtimeGraph")) else {
        return false;
    };
    if !ga.is_object() || !gb.is_object() {
        return false;
    }
    let id = |root: &Value| root.pointer("/release/id").cloned().unwrap_or(Value::Null);
    same_value(&id(&a), &id(&b)) && same_shape(ga, gb)
}

/// Equal JSON, numbers compared by value (1 and 1.0 are the same).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Equal JSON with every link blanked.
fn same_shape(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => without_link(x) == without_link(y),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same_shape(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, v)| y.get(key).is_some_and(|w| same_shape(v, w)))
        }
        _ => same_value(a, b),
    }
}

fn without_link(text: &str) -> &str {
    if is_link(text) {
        ""
    } else {
        text
    }
}

fn is_link(text: &str) -> bool {
    let scheme = text
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(
        scheme.as_deref(),
        Some("http" | "https" | "blob" | "data" | "file")
    ) || is_kept_path(text)
}

/// A file `Kept` names: an absolute path ending in
/// `files/<64 hex digits>.<extension>`.
fn is_kept_path(text: &str) -> bool {
    let path = std::path::Path::new(text);
    let bytes = text.as_bytes();
    let absolute = path.is_absolute()
        || text.starts_with('/')
        || text.starts_with("\\\\")
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'));
    let mut parts = text.rsplit(['/', '\\']);
    let (Some(name), Some(folder)) = (parts.next(), parts.next()) else {
        return false;
    };
    let digest = name.split('.').next().unwrap_or_default();
    absolute
        && folder == "files"
        && digest.len() == 64
        && digest.bytes().all(|b| b.is_ascii_hexdigit())
        && name.len() > 65
}

/// An RFC 3339 time (`2026-11-05T12:00:00.000Z`, fractions and offsets
/// allowed) as unix milliseconds; None before 1970.
fn unix_ms(text: &str) -> Option<u64> {
    let b = text.as_bytes();
    let number = |from: usize, to: usize| -> Option<i64> {
        let digits = b.get(from..to)?;
        digits.iter().all(u8::is_ascii_digit).then(|| {
            digits
                .iter()
                .fold(0i64, |n, d| n * 10 + i64::from(d - b'0'))
        })
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let (hour, minute, second) = (number(11, 13)?, number(14, 16)?, number(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    let mut millis = 0i64;
    if b[at] == b'.' {
        let start = at + 1;
        at = start;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return None;
        }
        let fraction = &text[start..at.min(start + 3)];
        millis = fraction.parse::<i64>().ok()? * 10i64.pow(3 - fraction.len() as u32);
    }
    let offset = match b.get(at..)? {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let minutes = number(at + 1, at + 3)? * 60 + number(at + 4, at + 6)?;
            if *sign == b'+' {
                minutes
            } else {
                -minutes
            }
        }
        _ => return None,
    };
    // Days since 1970-01-01 in the proleptic Gregorian calendar.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}
