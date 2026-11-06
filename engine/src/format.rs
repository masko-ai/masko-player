//! The release delivery, as `spec/rules.md` section 1 describes it. Only the
//! fields the rules read are decoded; players read media URLs, sounds and
//! movement from the same delivery themselves.

use serde::de::{Deserializer, IgnoredAny, Visitor};
use serde::Deserialize;
use std::collections::HashSet;
use std::fmt;

/// The rulebook version this engine implements (rules.md 1.2).
pub const PLAYER_CONTRACT: u64 = 1;
/// Features this engine implements (rules.md 1.2).
pub const FEATURES: &[&str] = &["any-state", "condition-groups"];

#[derive(Debug, PartialEq)]
pub struct LoadError {
    pub code: &'static str,
    pub detail: String,
}

fn refuse(code: &'static str, detail: impl Into<String>) -> LoadError {
    LoadError {
        code,
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawValue {
    Bool(bool),
    Number(f64),
}

/// A boolean or a number, read without buffering (an untagged enum costs
/// several kilobytes of WebAssembly).
impl<'de> Deserialize<'de> for RawValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = RawValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a boolean or a number")
            }
            fn visit_bool<E>(self, v: bool) -> Result<RawValue, E> {
                Ok(RawValue::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<RawValue, E> {
                Ok(RawValue::Number(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<RawValue, E> {
                Ok(RawValue::Number(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<RawValue, E> {
                Ok(RawValue::Number(v))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub input: String,
    pub op: Op,
    pub value: RawValue,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: String,
    /// None for an Any State edge (`source: "*"`).
    pub source: Option<String>,
    pub target: String,
    pub is_loop: bool,
    pub duration: f64,
    pub speed: f64,
    /// A number input that scales `speed` (rules.md 9.2).
    pub speed_input: Option<String>,
    pub priority: f64,
    /// Empty: no conditions. Otherwise any group whose conditions all hold.
    pub groups: Vec<Vec<Condition>>,
    /// The edge has a clip file to play (rules.md 1.4).
    pub has_video: bool,
    /// The edge has a sound file (rules.md 9.6), and whether it loops.
    pub sound: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct InputDecl {
    pub name: String,
    pub kind: String,
    pub default: RawValue,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub initial_node: String,
    pub nodes: Vec<String>,
    pub edges: Vec<Edge>,
    pub inputs: Vec<InputDecl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDelivery {
    runtime_graph: WireGraph,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireGraph {
    initial_node: String,
    nodes: Vec<WireNode>,
    edges: Vec<WireEdge>,
    #[serde(default)]
    inputs: Vec<WireInput>,
}

#[derive(Deserialize)]
struct WireNode {
    id: String,
}

#[derive(Deserialize, Default)]
struct WireVideos {
    webm: Option<String>,
    hevc: Option<String>,
    stacked: Option<String>,
    av1: Option<String>,
    /// A .mkpk pack, the file ESP32 boards play.
    mkpk: Option<String>,
    #[serde(default)]
    variants: WireVariants,
}

/// One size of `videos.variants`.
#[derive(Deserialize)]
struct WireFiles {
    webm: Option<String>,
    hevc: Option<String>,
    stacked: Option<String>,
    av1: Option<String>,
    mkpk: Option<String>,
}

/// `videos.variants`: true when one of its sizes names a file (rules.md 1.4).
#[derive(Default)]
struct WireVariants(bool);

impl<'de> Deserialize<'de> for WireVariants {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = WireVariants;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map of sizes")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> Result<WireVariants, A::Error> {
                let mut any = false;
                while let Some((_, f)) = m.next_entry::<IgnoredAny, WireFiles>()? {
                    any |= f.webm.is_some()
                        || f.hevc.is_some()
                        || f.stacked.is_some()
                        || f.av1.is_some()
                        || f.mkpk.is_some();
                }
                Ok(WireVariants(any))
            }
            fn visit_unit<E>(self) -> Result<WireVariants, E> {
                Ok(WireVariants(false))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct WireSound {
    url: Option<String>,
    #[serde(rename = "loop")]
    looping: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireEdge {
    id: String,
    source: String,
    target: String,
    is_loop: bool,
    duration: f64,
    speed: Option<f64>,
    speed_input: Option<String>,
    priority: Option<f64>,
    conditions: Option<Vec<WireCondition>>,
    condition_groups: Option<Vec<Vec<WireCondition>>>,
    #[serde(default)]
    videos: WireVideos,
    sound: Option<WireSound>,
}

#[derive(Deserialize)]
struct WireCondition {
    input: String,
    #[serde(default)]
    op: WireScalar,
    operator: Option<IgnoredAny>,
    #[serde(default)]
    value: WireScalar,
}

/// A condition's `op` or `value` as written, whatever its type, so a wrong
/// type is refused with its rule's code instead of as unreadable (rules.md
/// 1.3). `null` is `Other`; a field that is not there is `Missing`.
#[derive(Default)]
enum WireScalar {
    #[default]
    Missing,
    Bool(bool),
    Number(f64),
    Text(String),
    Other,
}

impl<'de> Deserialize<'de> for WireScalar {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = WireScalar;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_bool<E>(self, v: bool) -> Result<WireScalar, E> {
                Ok(WireScalar::Bool(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<WireScalar, E> {
                Ok(WireScalar::Number(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<WireScalar, E> {
                Ok(WireScalar::Number(v as f64))
            }
            fn visit_f64<E>(self, v: f64) -> Result<WireScalar, E> {
                Ok(WireScalar::Number(v))
            }
            fn visit_str<E>(self, v: &str) -> Result<WireScalar, E> {
                Ok(WireScalar::Text(v.to_owned()))
            }
            fn visit_unit<E>(self) -> Result<WireScalar, E> {
                Ok(WireScalar::Other)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, s: A) -> Result<WireScalar, A::Error> {
                skip(s).map(|_| WireScalar::Other)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, m: A) -> Result<WireScalar, A::Error> {
                skip_map(m).map(|_| WireScalar::Other)
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Deserialize)]
struct WireInput {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    default: RawValue,
}

/// The version fields alone, read whatever shape the rest of the delivery
/// has, so a newer delivery is refused for its version and not for a field
/// it changed (rules.md 1.2).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVersions {
    #[serde(default)]
    delivery_schema_version: AnyNumber,
    #[serde(default)]
    player_contract: AnyNumber,
    #[serde(default)]
    requires: AnyNames,
    #[serde(default)]
    runtime_graph: GraphVersion,
}

/// Any JSON value: the number when it is one.
#[derive(Default)]
struct AnyNumber(Option<f64>);

/// Any JSON value: a list of names when it is one, with `None` for an entry
/// that is not a string.
#[derive(Default)]
struct AnyNames(Option<Vec<Option<String>>>);

/// Any JSON value: `runtimeGraphSchemaVersion` when it is an object.
#[derive(Default)]
struct GraphVersion(Option<f64>);

/// Reads and drops whatever value comes next.
fn skip<'de, A: serde::de::SeqAccess<'de>>(mut seq: A) -> Result<(), A::Error> {
    while seq.next_element::<IgnoredAny>()?.is_some() {}
    Ok(())
}

fn skip_map<'de, A: serde::de::MapAccess<'de>>(mut map: A) -> Result<(), A::Error> {
    while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
    Ok(())
}

impl<'de> Deserialize<'de> for AnyNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = AnyNumber;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_u64<E>(self, v: u64) -> Result<AnyNumber, E> {
                Ok(AnyNumber(Some(v as f64)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<AnyNumber, E> {
                Ok(AnyNumber(Some(v as f64)))
            }
            fn visit_f64<E>(self, v: f64) -> Result<AnyNumber, E> {
                Ok(AnyNumber(Some(v)))
            }
            fn visit_bool<E>(self, _: bool) -> Result<AnyNumber, E> {
                Ok(AnyNumber(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<AnyNumber, E> {
                Ok(AnyNumber(None))
            }
            fn visit_unit<E>(self) -> Result<AnyNumber, E> {
                Ok(AnyNumber(None))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, s: A) -> Result<AnyNumber, A::Error> {
                skip(s).map(|_| AnyNumber(None))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, m: A) -> Result<AnyNumber, A::Error> {
                skip_map(m).map(|_| AnyNumber(None))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for AnyNames {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Name(Option<String>);
        impl<'de> Deserialize<'de> for Name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl<'de> Visitor<'de> for V {
                    type Value = Name;
                    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("any value")
                    }
                    fn visit_str<E>(self, v: &str) -> Result<Name, E> {
                        Ok(Name(Some(v.to_owned())))
                    }
                    fn visit_bool<E>(self, _: bool) -> Result<Name, E> {
                        Ok(Name(None))
                    }
                    fn visit_u64<E>(self, _: u64) -> Result<Name, E> {
                        Ok(Name(None))
                    }
                    fn visit_i64<E>(self, _: i64) -> Result<Name, E> {
                        Ok(Name(None))
                    }
                    fn visit_f64<E>(self, _: f64) -> Result<Name, E> {
                        Ok(Name(None))
                    }
                    fn visit_unit<E>(self) -> Result<Name, E> {
                        Ok(Name(None))
                    }
                    fn visit_seq<A: serde::de::SeqAccess<'de>>(
                        self,
                        s: A,
                    ) -> Result<Name, A::Error> {
                        skip(s).map(|_| Name(None))
                    }
                    fn visit_map<A: serde::de::MapAccess<'de>>(
                        self,
                        m: A,
                    ) -> Result<Name, A::Error> {
                        skip_map(m).map(|_| Name(None))
                    }
                }
                d.deserialize_any(V)
            }
        }
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = AnyNames;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut s: A,
            ) -> Result<AnyNames, A::Error> {
                let mut names = Vec::new();
                while let Some(Name(n)) = s.next_element::<Name>()? {
                    names.push(n);
                }
                Ok(AnyNames(Some(names)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, m: A) -> Result<AnyNames, A::Error> {
                skip_map(m).map(|_| AnyNames(Some(vec![None])))
            }
            fn visit_bool<E>(self, _: bool) -> Result<AnyNames, E> {
                Ok(AnyNames(Some(vec![None])))
            }
            fn visit_u64<E>(self, _: u64) -> Result<AnyNames, E> {
                Ok(AnyNames(Some(vec![None])))
            }
            fn visit_i64<E>(self, _: i64) -> Result<AnyNames, E> {
                Ok(AnyNames(Some(vec![None])))
            }
            fn visit_f64<E>(self, _: f64) -> Result<AnyNames, E> {
                Ok(AnyNames(Some(vec![None])))
            }
            fn visit_str<E>(self, _: &str) -> Result<AnyNames, E> {
                Ok(AnyNames(Some(vec![None])))
            }
            fn visit_unit<E>(self) -> Result<AnyNames, E> {
                Ok(AnyNames(None))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for GraphVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = GraphVersion;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any value")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut m: A,
            ) -> Result<GraphVersion, A::Error> {
                let mut version = None;
                while let Some(key) = m.next_key::<std::borrow::Cow<'de, str>>()? {
                    if key == "runtimeGraphSchemaVersion" {
                        version = m.next_value::<AnyNumber>()?.0;
                    } else {
                        m.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(GraphVersion(version))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                s: A,
            ) -> Result<GraphVersion, A::Error> {
                skip(s).map(|_| GraphVersion(None))
            }
            fn visit_bool<E>(self, _: bool) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
            fn visit_u64<E>(self, _: u64) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
            fn visit_i64<E>(self, _: i64) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
            fn visit_f64<E>(self, _: f64) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
            fn visit_str<E>(self, _: &str) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
            fn visit_unit<E>(self) -> Result<GraphVersion, E> {
                Ok(GraphVersion(None))
            }
        }
        d.deserialize_any(V)
    }
}

/// Reads a delivery, or refuses it with a reason (rules.md 1.2 and 1.3). The
/// versions are checked before anything else is read.
pub fn decode_delivery(json: &str) -> Result<Release, LoadError> {
    let versions: WireVersions =
        serde_json::from_str(json).map_err(|e| refuse("unreadable", e.to_string()))?;
    if versions.delivery_schema_version.0 != Some(1.0) {
        return Err(refuse(
            "unsupported_delivery",
            "deliverySchemaVersion must be 1",
        ));
    }
    match versions.player_contract.0 {
        None => {}
        Some(c) if c == PLAYER_CONTRACT as f64 => {}
        Some(c) => {
            return Err(refuse(
                "unsupported_contract",
                format!("playerContract {c}"),
            ))
        }
    }
    for name in versions.requires.0.unwrap_or_default() {
        match name {
            Some(n) if FEATURES.contains(&n.as_str()) => {}
            Some(n) => return Err(refuse("missing_feature", n)),
            None => {
                return Err(refuse(
                    "missing_feature",
                    "requires lists something that is not a name",
                ))
            }
        }
    }
    if versions.runtime_graph.0 != Some(1.0) {
        return Err(refuse(
            "unsupported_graph",
            "runtimeGraphSchemaVersion must be 1",
        ));
    }
    let wire: WireDelivery =
        serde_json::from_str(json).map_err(|e| refuse("unreadable", e.to_string()))?;
    decode_graph(wire.runtime_graph)
}

fn decode_graph(g: WireGraph) -> Result<Release, LoadError> {
    let mut nodes = HashSet::new();
    for n in &g.nodes {
        if !nodes.insert(n.id.as_str()) {
            return Err(refuse("invalid_graph", format!("duplicate node {}", n.id)));
        }
    }
    if !nodes.contains(g.initial_node.as_str()) {
        return Err(refuse("invalid_graph", "initialNode is not a node"));
    }
    let numbers: HashSet<&str> = g
        .inputs
        .iter()
        .filter(|i| i.kind == "number")
        .map(|i| i.name.as_str())
        .collect();
    let mut edge_ids = HashSet::new();
    let mut edges = Vec::with_capacity(g.edges.len());
    for e in g.edges {
        if !edge_ids.insert(e.id.clone()) {
            return Err(refuse("invalid_graph", format!("duplicate edge {}", e.id)));
        }
        let any_state = e.source == "*";
        if !nodes.contains(e.target.as_str()) || !(any_state || nodes.contains(e.source.as_str())) {
            return Err(refuse(
                "invalid_graph",
                format!("edge {} names a missing node", e.id),
            ));
        }
        if e.is_loop != (!any_state && e.source == e.target) {
            return Err(refuse(
                "invalid_graph",
                format!("edge {} isLoop disagrees", e.id),
            ));
        }
        let speed = e.speed.unwrap_or(1.0);
        // -0 ranks as 0, not below it (rules.md 4.1).
        let priority = match e.priority {
            Some(p) if p != 0.0 => p,
            _ => 0.0,
        };
        if !e.duration.is_finite()
            || e.duration < 0.0
            || !speed.is_finite()
            || speed <= 0.0
            || !priority.is_finite()
        {
            return Err(refuse("invalid_graph", format!("edge {} numbers", e.id)));
        }
        if let Some(name) = &e.speed_input {
            if !numbers.contains(name.as_str()) {
                return Err(refuse("invalid_graph", format!("edge {} speedInput", e.id)));
            }
        }
        // A non-empty conditionGroups replaces conditions; its empty groups
        // are dropped (rules.md 2.1).
        let raw_groups = match e.condition_groups {
            Some(groups) if !groups.is_empty() => {
                groups.into_iter().filter(|g| !g.is_empty()).collect()
            }
            _ => match e.conditions {
                Some(c) if !c.is_empty() => vec![c],
                _ => Vec::new(),
            },
        };
        let mut groups = Vec::with_capacity(raw_groups.len());
        for group in raw_groups {
            let mut out = Vec::with_capacity(group.len());
            for c in group {
                out.push(decode_condition(&e.id, c)?);
            }
            groups.push(out);
        }
        edges.push(Edge {
            id: e.id,
            source: if any_state { None } else { Some(e.source) },
            target: e.target,
            is_loop: e.is_loop,
            duration: e.duration,
            speed,
            speed_input: e.speed_input,
            priority,
            groups,
            has_video: e.videos.webm.is_some()
                || e.videos.hevc.is_some()
                || e.videos.stacked.is_some()
                || e.videos.av1.is_some()
                || e.videos.mkpk.is_some()
                || e.videos.variants.0,
            sound: e
                .sound
                .filter(|s| s.url.is_some())
                .map(|s| s.looping.unwrap_or(false)),
        });
    }
    let inputs = g
        .inputs
        .into_iter()
        .map(|i| InputDecl {
            name: i.name,
            kind: i.kind,
            default: i.default,
        })
        .collect();
    Ok(Release {
        initial_node: g.initial_node,
        nodes: g.nodes.into_iter().map(|n| n.id).collect(),
        edges,
        inputs,
    })
}

fn decode_condition(edge: &str, c: WireCondition) -> Result<Condition, LoadError> {
    if c.operator.is_some() {
        return Err(refuse(
            "unknown_condition_key",
            format!("edge {edge}: operator, not op"),
        ));
    }
    let text = match &c.op {
        WireScalar::Missing => "==",
        WireScalar::Text(t) => t.as_str(),
        _ => {
            return Err(refuse(
                "unknown_operator",
                format!("edge {edge}: op is not a string"),
            ))
        }
    };
    let op = match text {
        "==" => Op::Eq,
        "!=" => Op::Ne,
        ">" => Op::Gt,
        "<" => Op::Lt,
        ">=" => Op::Ge,
        "<=" => Op::Le,
        other => return Err(refuse("unknown_operator", format!("edge {edge}: {other}"))),
    };
    let value = match c.value {
        WireScalar::Missing => RawValue::Bool(true),
        WireScalar::Bool(b) => RawValue::Bool(b),
        WireScalar::Number(n) => RawValue::Number(n),
        _ => {
            return Err(refuse(
                "invalid_graph",
                format!("edge {edge}: a condition value is not a boolean or a number"),
            ))
        }
    };
    Ok(Condition {
        input: c.input,
        op,
        value,
    })
}
