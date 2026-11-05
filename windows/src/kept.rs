//! A canvas's latest release kept on the computer (spec/player.md, "A canvas
//! from its URL"), as the Mac player keeps it (`KeptRelease.swift`). Every
//! file is downloaded once and kept, named after its link's path: the path
//! never changes, only the signature does. A file is complete or not there.
//! `release.json` is the last release whose files were all kept, written
//! after them, so the mascot plays offline at the next launch.
//!
//! The kept files of a canvas live in
//! `<app data>/MaskoPlayer/<16 hex digits of the canvas URL's sha256>/files/`,
//! next to its `release.json`; `<app data>/MaskoPlayer/install-id` is the
//! installed app's id, whatever canvas it plays.
//!
//! `release.json` holds the release as the server gave it, with its links.
//! `kept_release` gives it with every link the player plays replaced by the
//! absolute path of its kept file, so the player reads files from disk only.

use crate::delivery::{self, Fetched};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

/// A canvas's kept files and release. Clones share their downloads in
/// flight, so the player makes one and gives clones to its threads.
#[derive(Debug, Clone)]
pub struct Kept {
    /// `<app data>/MaskoPlayer/<canvas>`: `release.json` and `files/`.
    folder: PathBuf,
    files: PathBuf,
    downloading: Arc<Downloading>,
}

/// The files being downloaded now: a second download of the same file waits
/// for the first and takes its file.
#[derive(Debug, Default)]
struct Downloading {
    files: Mutex<HashSet<PathBuf>>,
    done: Condvar,
}

/// The right to download a file, given back when dropped.
struct Turn<'a> {
    downloading: &'a Downloading,
    file: PathBuf,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut files = self
            .downloading
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        files.remove(&self.file);
        self.downloading.done.notify_all();
    }
}

/// The kept release, ready to play from disk.
#[derive(Debug, Clone)]
pub struct KeptRelease {
    pub release_id: String,
    pub number: u64,
    /// The delivery with every link the player plays (AV1 clips, stills and
    /// sounds) replaced by the absolute path of its kept file. Links it does
    /// not play (`webm`, `hevc`, `stacked`) stay as they are.
    pub delivery_json: String,
}

/// Why a file or the release could not be kept.
#[derive(Debug)]
pub enum KeepError {
    /// A file could not be downloaded whole.
    Network(String),
    /// A file could not be written, or the delivery cannot be read.
    Io(io::Error),
}

impl fmt::Display for KeepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeepError::Network(detail) => write!(f, "{detail}"),
            KeepError::Io(error) => write!(f, "cannot keep the release: {error}"),
        }
    }
}

impl std::error::Error for KeepError {}

impl From<io::Error> for KeepError {
    fn from(error: io::Error) -> Self {
        KeepError::Io(error)
    }
}

impl Kept {
    /// The kept files of the canvas at `canvas_url`. `app_data_dir` is the
    /// app's own folder (on Windows `%LOCALAPPDATA%\<app id>`). The folder is
    /// named after the URL without its query, so the same canvas asked with
    /// another `target` keeps its files. Nothing is read or written yet.
    pub fn new(app_data_dir: &Path, canvas_url: &str) -> Kept {
        let folder = root(app_data_dir).join(digest(&without_query(canvas_url), 16));
        Kept {
            files: folder.join("files"),
            folder,
            downloading: Arc::default(),
        }
    }

    /// The canvas's folder: `release.json` and `files/`.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// A random id this installed copy keeps, sent as `Masko-Install-Id` so
    /// the mascot's owner sees how many copies are active. Read from
    /// `<app data>/MaskoPlayer/install-id`, or made there (a lowercase UUID).
    pub fn install_id(app_data_dir: &Path) -> String {
        let root = root(app_data_dir);
        let file = root.join("install-id");
        if let Ok(saved) = fs::read_to_string(&file) {
            if is_uuid(&saved) {
                return saved;
            }
        }
        let id = new_uuid();
        if fs::create_dir_all(&root).is_ok() {
            let _ = write_atomically(&file, id.as_bytes());
        }
        id
    }

    /// The local file for a link: a digest of its path, with its extension.
    /// Fresh signatures of the same file give the same name.
    pub fn file_for(&self, link: &str) -> PathBuf {
        self.files.join(file_name(link))
    }

    /// Whether the file for a link is kept.
    pub fn has(&self, link: &str) -> bool {
        self.file_for(link).is_file()
    }

    /// The kept release, when every file it plays is here: what plays at
    /// launch, also offline.
    pub fn kept_release(&self) -> Option<KeptRelease> {
        let (release_id, number, delivery) = self.read_release()?;
        let links = delivery::links_in(&delivery).ok()?;
        if !links.iter().filter(|l| is_remote(l)).all(|l| self.has(l)) {
            return None;
        }
        Some(self.local(release_id, number, delivery, &links))
    }

    /// Deletes the kept files the kept release does not play, and pieces of
    /// downloads that never finished. The player calls it before it plays
    /// anything; without a kept release it deletes nothing.
    pub fn remove_unused(&self) {
        let Some((_, _, delivery)) = self.read_release() else {
            return;
        };
        let Ok(links) = delivery::links_in(&delivery) else {
            return;
        };
        if links.is_empty() {
            return;
        }
        let used: HashSet<String> = links.iter().map(|l| file_name(l)).collect();
        let Ok(entries) = fs::read_dir(&self.files) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !used.contains(name.to_string_lossy().as_ref()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// Downloads every file of `fetched` that is not here yet, one after the
    /// other, then keeps it as the release to play, also after the next
    /// launch. A file that fails stops it: the release kept before stays.
    pub fn keep(&self, fetched: &Fetched) -> Result<KeptRelease, KeepError> {
        let delivery: Value = serde_json::from_str(&fetched.delivery_json)
            .map_err(|e| KeepError::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?;
        let links = delivery::links_in(&delivery).map_err(|e| {
            KeepError::Io(io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        })?;
        let mut remote: Vec<&String> = links.iter().filter(|l| is_remote(l)).collect();
        remote.sort_by_key(|l| link_path(l));
        for link in remote {
            if !self.has(link) {
                self.download(link)?;
            }
        }
        fs::create_dir_all(&self.folder)?;
        let saved = json!({
            "release_id": fetched.release_id,
            "number": fetched.number,
            "delivery": delivery,
        });
        write_atomically(
            &self.folder.join("release.json"),
            saved.to_string().as_bytes(),
        )?;
        Ok(self.local(fetched.release_id.clone(), fetched.number, delivery, &links))
    }

    /// The kept file for a link, downloaded first when it is not here. The
    /// player plays the very first delivery from its links this way while
    /// `keep` runs. A local path (from a kept release) is returned as it is
    /// when the file exists. Several threads may ask at once: a link being
    /// downloaded already is waited for, not downloaded twice.
    pub fn fetch_file(&self, link: &str) -> Result<PathBuf, KeepError> {
        if !is_remote(link) {
            let path = PathBuf::from(link);
            return if path.is_file() {
                Ok(path)
            } else {
                Err(KeepError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{link} is not there"),
                )))
            };
        }
        let file = self.file_for(link);
        if file.is_file() {
            return Ok(file);
        }
        self.download(link)
    }

    /// The release in `release.json`: its id, number and delivery.
    fn read_release(&self) -> Option<(String, u64, Value)> {
        let text = fs::read_to_string(self.folder.join("release.json")).ok()?;
        let mut saved: Value = serde_json::from_str(&text).ok()?;
        let delivery = saved.get_mut("delivery").filter(|d| d.is_object())?.take();
        let release_id = saved.get("release_id").and_then(Value::as_str)?.to_owned();
        let number = saved.get("number").and_then(Value::as_u64)?;
        Some((release_id, number, delivery))
    }

    /// The release with each link it plays replaced by its kept file.
    fn local(
        &self,
        release_id: String,
        number: u64,
        mut delivery: Value,
        links: &[String],
    ) -> KeptRelease {
        let paths: HashMap<&str, String> = links
            .iter()
            .filter(|l| is_remote(l))
            .map(|l| (l.as_str(), self.file_for(l).to_string_lossy().into_owned()))
            .collect();
        if let Some(graph) = delivery.get_mut("runtimeGraph") {
            replace_links(graph, &paths);
        }
        KeptRelease {
            release_id,
            number,
            delivery_json: delivery.to_string(),
        }
    }

    /// Downloads a link next to its kept file, then renames it into place, so
    /// a file is either complete or not there.
    fn download(&self, link: &str) -> Result<PathBuf, KeepError> {
        let destination = self.file_for(link);
        let _turn = self.turn(&destination);
        if destination.is_file() {
            return Ok(destination);
        }
        fs::create_dir_all(&self.files)?;
        let part = self.files.join(format!("{}.part", random_hex(16)));
        let result = save(link, &part).and_then(|()| place(&part, &destination));
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result.map(|()| destination)
    }

    /// Waits until no other thread downloads `file`, then holds it.
    fn turn(&self, file: &Path) -> Turn<'_> {
        let downloading = &*self.downloading;
        let mut files = downloading
            .files
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while files.contains(file) {
            files = downloading
                .done
                .wait(files)
                .unwrap_or_else(PoisonError::into_inner);
        }
        files.insert(file.to_path_buf());
        Turn {
            downloading,
            file: file.to_path_buf(),
        }
    }
}

/// Downloads a link into `part`: a 2xx answer, not empty, as long as it said.
fn save(link: &str, part: &Path) -> Result<(), KeepError> {
    let name = last_component(link);
    let mut response = delivery::agent()
        .get(link)
        .call()
        .map_err(|e| KeepError::Network(format!("{name} cannot be downloaded: {e}")))?;
    let status = response.status().as_u16();
    let incomplete = || KeepError::Network(format!("{name} arrived incomplete ({status})"));
    if !(200..300).contains(&status) {
        return Err(incomplete());
    }
    let expected = response.body().content_length();
    let mut reader = response.body_mut().as_reader();
    let mut file = fs::File::create(part)?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(KeepError::Network(format!(
                    "{name} arrived incomplete: {e}"
                )))
            }
        };
        file.write_all(&buffer[..read])?;
        size += read as u64;
    }
    if size == 0 || expected.is_some_and(|expected| expected != size) {
        return Err(incomplete());
    }
    // On disk before the rename, so a crash never leaves a named file that
    // is not whole.
    file.sync_all()?;
    Ok(())
}

/// Renames a finished download into place. When another download of the
/// same link got there first (another process), its file stays: it may be
/// playing, and Windows cannot replace a file that is open.
fn place(part: &Path, destination: &Path) -> Result<(), KeepError> {
    if destination.is_file() {
        let _ = fs::remove_file(part);
        return Ok(());
    }
    match rename(part, destination) {
        Ok(()) => Ok(()),
        Err(_) if destination.is_file() => {
            let _ = fs::remove_file(part);
            Ok(())
        }
        Err(e) => Err(KeepError::Io(e)),
    }
}

/// Renames a file, trying again for a moment: on Windows a virus scanner
/// often holds a file it just saw being written.
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let mut wait = Duration::from_millis(20);
    for _ in 0..4 {
        if fs::rename(from, to).is_ok() {
            return Ok(());
        }
        std::thread::sleep(wait);
        wait *= 2;
    }
    fs::rename(from, to)
}

/// Writes a file whole or not at all: next to it first, then renamed.
fn write_atomically(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let folder = file.parent().unwrap_or_else(|| Path::new("."));
    let part = folder.join(format!("{}.part", random_hex(16)));
    let result = (|| {
        let mut out = fs::File::create(&part)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        drop(out);
        rename(&part, file)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Replaces every string of `value` that is a key of `paths`.
fn replace_links(value: &mut Value, paths: &HashMap<&str, String>) {
    match value {
        Value::String(text) => {
            if let Some(path) = paths.get(text.as_str()) {
                *text = path.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| replace_links(v, paths)),
        Value::Object(fields) => fields.values_mut().for_each(|v| replace_links(v, paths)),
        _ => {}
    }
}

/// `<app data>/MaskoPlayer`, absolute so kept paths are.
fn root(app_data_dir: &Path) -> PathBuf {
    std::path::absolute(app_data_dir)
        .unwrap_or_else(|_| app_data_dir.to_path_buf())
        .join("MaskoPlayer")
}

/// A link the player downloads; anything else is already a local file.
fn is_remote(link: &str) -> bool {
    let lower = link.get(..8).unwrap_or(link).to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// The URL without its query (`?...`); a fragment stays, as on the Mac.
fn without_query(url: &str) -> String {
    match url.find('?') {
        Some(start) => {
            let end = url[start..].find('#').map_or(url.len(), |at| start + at);
            format!("{}{}", &url[..start], &url[end..])
        }
        None => url.to_owned(),
    }
}

/// The file name for a link: the sha256 of its percent-decoded path, with
/// the path's extension (`bin` when it has none, or one Windows cannot name).
fn file_name(link: &str) -> String {
    let path = link_path(link);
    let last = path.rsplit('/').next().unwrap_or_default();
    let extension = match last.rfind('.') {
        Some(at) if at > 0 => &last[at + 1..],
        _ => "",
    };
    let extension = if !extension.is_empty()
        && extension.len() <= 16
        && extension.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        extension
    } else {
        "bin"
    };
    format!("{}.{extension}", digest(&path, 64))
}

/// A link's path, percent-decoded, without its query or fragment, as
/// Foundation's `URL.path` reads it.
fn link_path(link: &str) -> String {
    let rest = match link.find("://") {
        Some(at) => {
            let after = &link[at + 3..];
            after.find('/').map_or("", |slash| &after[slash..])
        }
        None => link,
    };
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    percent_decode(&rest[..end])
}

/// The last part of a link's path, for messages; never its query.
fn last_component(link: &str) -> String {
    let path = link_path(link);
    path.rsplit('/').next().unwrap_or_default().to_owned()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn digest(text: &str, length: usize) -> String {
    let hash = Sha256::digest(text.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    hex[..length].to_owned()
}

fn random_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Never seen on Windows or macOS; the time and a counter still make
        // a name no other download uses.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let seed = format!(
            "{now}-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        );
        bytes.copy_from_slice(&Sha256::digest(seed.as_bytes())[..16]);
    }
    bytes
}

fn random_hex(length: usize) -> String {
    let hex: String = random_bytes().iter().map(|b| format!("{b:02x}")).collect();
    hex[..length.min(32)].to_owned()
}

/// A random (version 4) UUID, lowercase.
fn new_uuid() -> String {
    let mut b = random_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `8-4-4-4-12` hex digits, as Foundation's `UUID(uuidString:)` reads one.
fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}
