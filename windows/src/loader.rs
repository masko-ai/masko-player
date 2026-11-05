//! Gets a release's files ready off the player's thread: a clip, still or
//! sound link becomes a kept local file (downloaded once, kept for good, see
//! `kept.rs`), and a sound file becomes samples. Results come back in order
//! of completion and the player picks them up on its next turn.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::kept::Kept;
use crate::media::{sound_file, Pcm};

/// How many files download at once.
const WORKERS: usize = 2;

enum Job {
    File { id: u64, link: String },
    Sound { id: u64, link: String },
}

pub(crate) enum Done {
    File {
        id: u64,
        result: Result<PathBuf, String>,
    },
    Sound {
        id: u64,
        result: Result<Pcm, String>,
    },
}

pub(crate) struct Loader {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// Results found without a thread (a file already on the computer).
    ready: Vec<Done>,
    next: u64,
}

impl Loader {
    pub fn new(kept: Option<Arc<Kept>>) -> Loader {
        let (jobs, queue) = channel::<Job>();
        let (finished, done) = channel::<Done>();
        let queue = Arc::new(Mutex::new(queue));
        for i in 0..WORKERS {
            let queue = Arc::clone(&queue);
            let finished = finished.clone();
            let kept = kept.clone();
            let _ = std::thread::Builder::new()
                .name(format!("masko-files-{i}"))
                .spawn(move || loop {
                    let job = match queue.lock().unwrap().recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    let done = match job {
                        Job::File { id, link } => Done::File {
                            id,
                            result: local(&kept, &link),
                        },
                        Job::Sound { id, link } => Done::Sound {
                            id,
                            result: local(&kept, &link)
                                .and_then(|path| sound_file(&path).map_err(|e| e.to_string())),
                        },
                    };
                    if finished.send(done).is_err() {
                        return;
                    }
                });
        }
        Loader {
            jobs,
            done,
            ready: Vec::new(),
            next: 0,
        }
    }

    /// Asks for a link's local file; the answer comes from `take` with the
    /// returned id.
    pub fn file(&mut self, link: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        if let Some(path) = on_disk(link) {
            self.ready.push(Done::File {
                id,
                result: Ok(path),
            });
        } else {
            let _ = self.jobs.send(Job::File {
                id,
                link: link.to_string(),
            });
        }
        id
    }

    /// Asks for a sound link's samples.
    pub fn sound(&mut self, link: &str) -> u64 {
        self.next += 1;
        let id = self.next;
        let _ = self.jobs.send(Job::Sound {
            id,
            link: link.to_string(),
        });
        id
    }

    /// Everything finished since the last call.
    pub fn take(&mut self) -> Vec<Done> {
        let mut out = std::mem::take(&mut self.ready);
        out.extend(self.done.try_iter());
        out
    }
}

/// A link that is already a file on the computer (a kept release names its
/// files by their local path).
fn on_disk(link: &str) -> Option<PathBuf> {
    let path = Path::new(link.strip_prefix("file://").unwrap_or(link));
    (path.is_absolute() && path.is_file()).then(|| path.to_path_buf())
}

fn local(kept: &Option<Arc<Kept>>, link: &str) -> Result<PathBuf, String> {
    if let Some(path) = on_disk(link) {
        return Ok(path);
    }
    match kept {
        Some(kept) => kept.fetch_file(link).map_err(|e| e.to_string()),
        None => Err(format!("no local file for {link}")),
    }
}
