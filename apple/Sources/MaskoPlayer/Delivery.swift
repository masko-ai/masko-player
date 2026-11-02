import Foundation

/// Any JSON value, as the delivery gives it. The player hands an edge's
/// `movement` to the app this way, unchanged.
public enum MaskoJSON: Equatable, Sendable, Decodable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([MaskoJSON])
    case object([String: MaskoJSON])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([MaskoJSON].self) {
            self = .array(a)
        } else {
            self = .object(try c.decode([String: MaskoJSON].self))
        }
    }
}

/// What the player reads from a release delivery for itself: files, sounds,
/// movement, the poses each edge joins (to decode a move's next loop early)
/// and when signed links expire. The engine reads the rules from the same
/// JSON.
struct Delivery {
    struct Sound: Equatable {
        var url: URL
        var volume: Double
        var loop: Bool
    }

    struct Edge {
        /// The pose it leaves (`*` for any pose) and the pose it reaches.
        var source: String?
        var target: String?
        /// A pose's own clip, which loops on its pose.
        var isLoop: Bool
        /// The base file: stacked AV1 when the clip has one (a delivery asked
        /// with `formats=av1`), else HEVC with alpha.
        var file: URL?
        /// Size variants of that kind, smallest first (manifest mode).
        var variants: [(size: Int, file: URL)]
        var sound: Sound?
        var movement: MaskoJSON?
        var hasAudio: Bool
        /// The clip's length in seconds at rate 1.
        var duration: Double
    }

    var edges: [String: Edge]
    /// The pose the mascot starts on.
    var initialNode: String?
    /// Each pose's `transparentThumbnailUrl`.
    var stills: [String: URL]
    var autoPlay: Bool
    /// `deliveryAccess.signedUrlExpiresAt`, for signed links only.
    var expiresAt: Date?
    /// The release with its links left out: `release.id` and the graph.
    let shape: Data?

    init(json: Data) throws {
        let wire = try JSONDecoder().decode(Wire.self, from: json)
        var edges: [String: Edge] = [:]
        for e in wire.runtimeGraph.edges {
            let videos = e.videos?.value
            let sizes = videos?.variants?.value ?? [:]
            // A clip with an AV1 file plays it, at its base and in every size.
            let av1 = videos?.av1?.url != nil || sizes.values.contains { $0.value?.av1?.url != nil }
            let variants = sizes
                .compactMap { key, files -> (Int, URL)? in
                    let file = av1 ? files.value?.av1?.url : files.value?.hevc?.url
                    guard let size = Int(key), let file else { return nil }
                    return (size, file)
                }
                .sorted { $0.0 < $1.0 }
                .map { (size: $0.0, file: $0.1) }
            let sound = e.sound?.value.flatMap { s in
                s.url?.url.map {
                    Sound(url: $0, volume: min(max(s.volume?.value ?? 1, 0), 1), loop: s.loop?.value ?? false)
                }
            }
            edges[e.id] = Edge(
                source: e.source?.value,
                target: e.target?.value,
                isLoop: e.isLoop?.value ?? false,
                file: av1 ? videos?.av1?.url : videos?.hevc?.url,
                variants: variants,
                sound: sound,
                movement: e.movement,
                hasAudio: e.hasAudio?.value ?? false,
                duration: e.duration?.value ?? 0
            )
        }
        self.edges = edges
        var stills: [String: URL] = [:]
        for n in wire.runtimeGraph.nodes {
            stills[n.id] = n.transparentThumbnailUrl?.url
        }
        self.stills = stills
        initialNode = wire.runtimeGraph.initialNode?.value
        autoPlay = wire.runtimeGraph.autoPlay?.value ?? true
        if let access = wire.deliveryAccess?.value, access.urlMode?.value == "signed" {
            expiresAt = access.signedUrlExpiresAt?.value.flatMap(Delivery.date)
        } else {
            expiresAt = nil
        }
        shape = Delivery.shape(of: json)
    }

    /// The delivery a lone clip plays as (player.md, "One clip"): one pose,
    /// `clip`, whose loop is the file (HEVC, or stacked AV1), `seconds` long,
    /// with its own voice when the file has a sound track (`voiced`).
    static func clip(file: URL, seconds: TimeInterval, voiced: Bool) -> Data {
        let delivery: [String: Any] = [
            "deliverySchemaVersion": 1,
            "playerContract": 1,
            "requires": [String](),
            "runtimeGraph": [
                "runtimeGraphSchemaVersion": 1,
                "initialNode": "clip",
                "nodes": [["id": "clip"]],
                "edges": [[
                    "id": "clip", "source": "clip", "target": "clip", "isLoop": true,
                    "duration": seconds, "videos": ["hevc": file.absoluteString], "hasAudio": voiced,
                ]],
                "inputs": [Any](),
            ],
        ]
        return (try? JSONSerialization.data(withJSONObject: delivery)) ?? Data()
    }

    /// The file to play for an edge on a mascot `pixels` wide on screen: the
    /// smallest variant at least that large, else the base file (player.md).
    /// `key` names the choice, so a cached file outlives a link refresh.
    func file(edge id: String, pixels: Double?) -> (url: URL, key: String)? {
        guard let edge = edges[id] else { return nil }
        if let pixels, let v = edge.variants.first(where: { Double($0.size) >= pixels }) {
            return (v.file, "\(id)@\(v.size)")
        }
        if let file = edge.file {
            return (file, id)
        }
        if let largest = edge.variants.last {
            return (largest.file, "\(id)@\(largest.size)")
        }
        return nil
    }

    /// Whether `fresh` is the same release, so that only its links may
    /// differ: the same `release.id` and the same graph, links left out.
    func sameRelease(as fresh: Delivery) -> Bool {
        shape != nil && shape == fresh.shape
    }

    /// Takes the links of a fresh delivery of the same release.
    mutating func takeLinks(from fresh: Delivery) {
        for (id, edge) in fresh.edges where edges[id] != nil {
            edges[id]?.file = edge.file
            edges[id]?.variants = edge.variants
            edges[id]?.sound = edge.sound
        }
        for (id, url) in fresh.stills {
            stills[id] = url
        }
        expiresAt = fresh.expiresAt
    }

    /// `[release.id, runtimeGraph]` as JSON with sorted keys, every link
    /// blanked, so two deliveries of the same release give the same bytes.
    static func shape(of json: Data) -> Data? {
        guard let root = (try? JSONSerialization.jsonObject(with: json)) as? [String: Any] else { return nil }
        let release = (root["release"] as? [String: Any])?["id"] ?? NSNull()
        let graph = withoutLinks(root["runtimeGraph"] ?? NSNull())
        return try? JSONSerialization.data(withJSONObject: [release, graph], options: [.sortedKeys])
    }

    private static func withoutLinks(_ value: Any) -> Any {
        switch value {
        case let text as String:
            let link = text.range(of: "^(https?|blob|data|file):", options: [.regularExpression, .caseInsensitive])
            return link == nil ? text : ""
        case let values as [Any]:
            return values.map(withoutLinks)
        case let values as [String: Any]:
            return values.mapValues(withoutLinks)
        default:
            return value
        }
    }

    static func date(_ text: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: text) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: text)
    }

    // Only `id`s are required; a media field of the wrong type is ignored
    // rather than refusing a delivery the engine accepts.

    private struct Wire: Decodable {
        var runtimeGraph: Graph
        var deliveryAccess: Lenient<Access>?
    }

    private struct Access: Decodable {
        var urlMode: Lenient<String>?
        var signedUrlExpiresAt: Lenient<String>?
    }

    private struct Graph: Decodable {
        var initialNode: Lenient<String>?
        var autoPlay: Lenient<Bool>?
        var nodes: [Node]
        var edges: [WireEdge]
    }

    private struct Node: Decodable {
        var id: String
        var transparentThumbnailUrl: Lenient<String>?
    }

    private struct Files: Decodable {
        var hevc: Lenient<String>?
        var av1: Lenient<String>?
    }

    private struct Videos: Decodable {
        var hevc: Lenient<String>?
        var av1: Lenient<String>?
        var variants: Lenient<[String: Lenient<Files>]>?
    }

    private struct WireSound: Decodable {
        var url: Lenient<String>?
        var volume: Lenient<Double>?
        var loop: Lenient<Bool>?
    }

    private struct WireEdge: Decodable {
        var id: String
        var source: Lenient<String>?
        var target: Lenient<String>?
        var isLoop: Lenient<Bool>?
        var videos: Lenient<Videos>?
        var sound: Lenient<WireSound>?
        var movement: MaskoJSON?
        var hasAudio: Lenient<Bool>?
        var duration: Lenient<Double>?
    }
}

/// A value when it has the expected type, and nothing otherwise.
private struct Lenient<T: Decodable>: Decodable {
    var value: T?

    init(from decoder: Decoder) throws {
        value = try? T(from: decoder)
    }
}

extension Lenient where T == String {
    var url: URL? { value.flatMap(URL.init(string:)) }
}
