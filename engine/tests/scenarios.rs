//! Runs every scenario in `spec/fixtures` against the engine, through an
//! ideal player: every clip loads at once unless a step makes it fail, plays
//! for its file's length divided by its rate (whole milliseconds, halves to
//! even), and is swapped exactly when the engine asks (spec/fixtures/README.md).

use masko_player_engine::{load, Command, Engine, Value as V};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../spec/fixtures")
}

fn merge(base: &mut Value, patch: &Value) {
    let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) else {
        return;
    };
    for (k, v) in p {
        if v.is_null() {
            b.remove(k);
        } else if v.is_object() && b.get(k).is_some_and(Value::is_object) {
            merge(b.get_mut(k).unwrap(), v);
        } else {
            b.insert(k.clone(), v.clone());
        }
    }
}

fn build_delivery(fx: &Value) -> Value {
    let mut delivery = match &fx["canvas"] {
        Value::String(name) => {
            let text = fs::read_to_string(fixtures().join("canvases").join(format!("{name}.json")))
                .expect("canvas file");
            serde_json::from_str(&text).unwrap()
        }
        inline => inline.clone(),
    };
    let patch = fx.get("patch").cloned().unwrap_or(json!({}));
    if let Some(d) = patch.get("delivery") {
        merge(&mut delivery, d);
    }
    let g = delivery["runtimeGraph"].as_object_mut().unwrap();
    if let Some(edges) = patch.get("edges").and_then(Value::as_object) {
        for (id, change) in edges {
            let list = g["edges"].as_array_mut().unwrap();
            let idx = list
                .iter()
                .position(|e| e["id"] == *id)
                .expect("patched edge");
            if change.is_null() {
                list.remove(idx);
            } else {
                merge(&mut list[idx], change);
            }
        }
    }
    if let Some(add) = patch.get("addEdges").and_then(Value::as_array) {
        g["edges"]
            .as_array_mut()
            .unwrap()
            .extend(add.iter().cloned());
    }
    if !g.contains_key("inputs") {
        g.insert("inputs".into(), json!([]));
    }
    if let Some(add) = patch.get("addInputs").and_then(Value::as_array) {
        g["inputs"]
            .as_array_mut()
            .unwrap()
            .extend(add.iter().cloned());
    }
    if let Some(defaults) = patch.get("defaults").and_then(Value::as_object) {
        for (name, d) in defaults {
            let inputs = g["inputs"].as_array_mut().unwrap();
            let input = inputs
                .iter_mut()
                .find(|i| i["name"] == *name)
                .expect("input");
            input["default"] = d.clone();
        }
    }
    delivery
}

fn round_ms(x: f64) -> u64 {
    x.round_ties_even().max(0.0) as u64
}

/// The ideal player around the engine.
struct Player {
    engine: Engine,
    /// Each edge's file length in seconds.
    files: HashMap<String, f64>,
    durations: HashMap<String, f64>,
    no_end: HashSet<String>,
    /// How long some edges' first frames take to show, in milliseconds.
    load: HashMap<String, u64>,
    /// The clip the player has on screen: edge, shown at, rate.
    clip: Option<(String, u64, f64)>,
    /// A clip asked for and not yet reported shown: edge, when it shows.
    showing: Option<(String, u64)>,
    events: Vec<String>,
    preloaded: Vec<String>,
}

impl Player {
    fn absorb(&mut self, commands: Vec<Command>) {
        let now = self.engine.clock();
        for c in commands {
            match c {
                Command::ShowClip { edge, rate, .. } => {
                    self.events.push(format!("shown:{edge}"));
                    let at = now + self.load.get(&edge).copied().unwrap_or(0);
                    self.clip = Some((edge.clone(), at, rate));
                    self.showing = Some((edge, at));
                }
                Command::ShowStill { pose } => {
                    self.events.push(format!("shown:still:{pose}"));
                    self.clip = None;
                    self.showing = None;
                }
                Command::Preload { edge } => self.preloaded.push(edge),
                Command::PoseEntered { pose } => self.events.push(format!("pose:{pose}")),
                Command::Interrupted { edge } => self.events.push(format!("interrupted:{edge}")),
                Command::Spent { name, used } => self.events.push(if used {
                    format!("spent:{name}")
                } else {
                    format!("spent:{name}:unused")
                }),
                Command::Error { code, .. } => self.events.push(format!("error:{code}")),
                Command::PlaySound { .. } | Command::StopSounds { .. } => {}
            }
        }
        // A clip that shows at once is reported shown at once.
        if let Some((edge, at)) = self.showing.clone() {
            if at == now {
                self.showing = None;
                let commands = self.engine.clip_shown(&edge, now);
                self.absorb(commands);
            }
        }
    }

    /// When the player reports the end of its clip, unless a step took the
    /// end signal away.
    fn clip_end(&self) -> Option<(String, u64)> {
        let (edge, shown, rate) = self.clip.as_ref()?;
        if self.no_end.contains(edge) {
            return None;
        }
        let len = self
            .files
            .get(edge)
            .copied()
            .unwrap_or(self.durations[edge]);
        Some((edge.clone(), shown + round_ms(len / rate * 1000.0)))
    }

    fn advance_to(&mut self, until: u64) {
        for _ in 0..100_000 {
            if let Some((edge, at)) = self.showing.clone() {
                if at <= until && self.engine.next_wake().is_none_or(|w| at <= w) {
                    self.showing = None;
                    let commands = self.engine.clip_shown(&edge, at);
                    self.absorb(commands);
                    continue;
                }
            }
            let end = self.clip_end();
            let wake = self.engine.next_wake();
            let next = match (&end, wake) {
                (Some((_, e)), Some(w)) => Some((*e).min(w)),
                (Some((_, e)), None) => Some(*e),
                (None, w) => w,
            };
            match next {
                Some(t) if t <= until => {
                    let commands = match &end {
                        Some((edge, e)) if *e <= wake.unwrap_or(u64::MAX) => {
                            let state = self.engine.state();
                            assert_eq!(
                                state.clip.as_deref(),
                                Some(edge.as_str()),
                                "the player and the engine disagree on the clip on screen"
                            );
                            self.engine.clip_ended(edge, *e)
                        }
                        _ => self.engine.advance(t),
                    };
                    self.absorb(commands);
                }
                _ => {
                    let commands = self.engine.advance(until);
                    self.absorb(commands);
                    return;
                }
            }
        }
        panic!("the clock did not settle");
    }

    fn run_to_end(&mut self, moving: bool) {
        let state = self.engine.state();
        assert_eq!(
            state.moving, moving,
            "loopEnded or clipEnded on the wrong kind of clip"
        );
        let safety = state.shown_at + state.expected_ms + 1000;
        let until = if state
            .clip
            .as_deref()
            .is_some_and(|c| c.starts_with("still:"))
        {
            state.shown_at + state.expected_ms
        } else {
            match self.clip_end() {
                Some((_, e)) => e.min(safety),
                None => safety,
            }
        };
        self.advance_to(until);
    }
}

fn value(v: &Value) -> V {
    match v {
        Value::Bool(b) => V::Bool(*b),
        Value::Number(n) => V::Number(n.as_f64().unwrap()),
        other => panic!("not a value: {other}"),
    }
}

fn act(p: &mut Player, step: &Value) {
    let now = p.engine.clock();
    // `"instant": true` on a set or fire step (rules.md 5.8).
    let instant = step
        .get("instant")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let commands = if step.get("start").is_some() {
        p.engine.start(now)
    } else if let Some(s) = step.get("set") {
        p.engine
            .set(s[0].as_str().unwrap(), value(&s[1]), instant, now)
    } else if let Some(f) = step.get("fire") {
        p.engine.fire(f.as_str().unwrap(), instant, now)
    } else if let Some(ms) = step.get("advance") {
        p.advance_to(now + ms.as_u64().unwrap());
        Vec::new()
    } else if step.get("loopEnded").is_some() {
        p.run_to_end(false);
        Vec::new()
    } else if step.get("clipEnded").is_some() {
        p.run_to_end(true);
        Vec::new()
    } else if let Some(e) = step.get("failClip") {
        p.engine.mark_unplayable(e.as_str().unwrap());
        Vec::new()
    } else if let Some(e) = step.get("loadsAgain") {
        p.engine.mark_playable(e.as_str().unwrap());
        Vec::new()
    } else if let Some(e) = step.get("noEndSignal") {
        p.no_end.insert(e.as_str().unwrap().to_owned());
        Vec::new()
    } else if step.get("reset").is_some() {
        p.engine.reset(now)
    } else if let Some(pose) = step.get("showPose") {
        p.engine.preview_show_pose(pose.as_str().unwrap(), now)
    } else if let Some(e) = step.get("playMove") {
        p.engine.preview_play_move(e.as_str().unwrap(), now)
    } else {
        panic!("unknown step {step}");
    };
    p.absorb(commands);
}

fn as_json(v: V) -> Value {
    match v {
        V::Bool(b) => json!(b),
        V::Number(n) => json!(n),
    }
}

fn observed(p: &Player, want: &Map<String, Value>) -> Map<String, Value> {
    let s = p.engine.state();
    let mut out = Map::new();
    out.insert("clock".into(), json!(s.clock));
    out.insert("pose".into(), json!(s.pose));
    out.insert("clip".into(), json!(s.clip));
    out.insert(
        "next".into(),
        match s.next {
            None => Value::Null,
            Some(n) => json!({ "clip": n.clip, "at": n.at }),
        },
    );
    out.insert("events".into(), json!(p.events));
    if let Some(names) = want.get("values").and_then(Value::as_object) {
        let values: Map<String, Value> = names
            .keys()
            .map(|n| (n.clone(), as_json(p.engine.value(n))))
            .collect();
        out.insert("values".into(), Value::Object(values));
    }
    if want.contains_key("fired") {
        out.insert("fired".into(), json!(p.engine.pending_fires()));
    }
    if want.contains_key("preloaded") {
        out.insert("preloaded".into(), json!(p.preloaded));
    }
    out
}

/// Equality as the scenarios mean it: a boolean equals the number 1 or 0.
fn same(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| match v {
        Value::Bool(x) => Some(f64::from(u8::from(*x))),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

fn run(path: &Path) -> Vec<String> {
    let fx: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let delivery = build_delivery(&fx);
    let want_refusal = fx
        .get("expectLoad")
        .and_then(|e| e.get("refused"))
        .and_then(Value::as_str);
    let engine = match load(&delivery.to_string()) {
        Err(e) => {
            return if want_refusal == Some(e.code) {
                Vec::new()
            } else {
                vec![format!(
                    "load refused with {}, expected {:?}",
                    e.code, want_refusal
                )]
            };
        }
        Ok(engine) => engine,
    };
    if let Some(code) = want_refusal {
        return vec![format!("loaded, expected refusal {code}")];
    }
    let mut engine = engine;
    let rolls: Vec<f64> = fx
        .get("random")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect())
        .unwrap_or_default();
    engine.use_rolls(rolls);
    let files: HashMap<String, f64> = fx
        .get("files")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_f64().unwrap()))
                .collect()
        })
        .unwrap_or_default();
    let load: HashMap<String, u64> = fx
        .get("loadMs")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .map(|(k, v)| (k.clone(), v.as_u64().unwrap()))
                .collect()
        })
        .unwrap_or_default();
    let durations: HashMap<String, f64> = delivery["runtimeGraph"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["duration"].as_f64().unwrap(),
            )
        })
        .collect();
    let mut player = Player {
        engine,
        files,
        durations,
        no_end: HashSet::new(),
        load,
        clip: None,
        showing: None,
        events: Vec::new(),
        preloaded: Vec::new(),
    };
    let mut failures = Vec::new();
    for (i, step) in fx["steps"].as_array().unwrap().iter().enumerate() {
        player.events.clear();
        player.preloaded.clear();
        act(&mut player, step);
        let want = step
            .get("expect")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let got = observed(&player, &want);
        for (k, v) in &want {
            let g = got.get(k).cloned().unwrap_or(Value::Null);
            if !same(&g, v) {
                failures.push(format!("step {} {k}: got {g}, expected {v}", i + 1));
            }
        }
    }
    failures
}

#[test]
fn every_scenario_agrees_with_the_rulebook() {
    let mut paths: Vec<PathBuf> = fs::read_dir(fixtures())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let mut bad = 0;
    for p in &paths {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let result = std::panic::catch_unwind(|| run(p));
        match result {
            Ok(f) if f.is_empty() => println!("ok   {name}"),
            Ok(f) => {
                bad += 1;
                println!("FAIL {name}");
                for line in f {
                    println!("     {line}");
                }
            }
            Err(_) => {
                bad += 1;
                println!("FAIL {name} (panicked)");
            }
        }
    }
    println!("{} of {} scenarios pass", paths.len() - bad, paths.len());
    assert_eq!(bad, 0, "{bad} scenarios failed");
}
