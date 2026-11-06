import Foundation
import XCTest

@testable import MaskoPlayer

/// Runs every scenario in `spec/fixtures` through the Swift binding with the
/// ideal player of `spec/fixtures/README.md`, as `engine/tests/scenarios.rs`
/// does in Rust: every clip loads and shows at once, reported with
/// `clipShown`, unless a step makes it fail or `loadMs` delays it, and plays
/// for its file's length divided by its rate (whole milliseconds, halves to
/// even).
final class ScenarioTests: XCTestCase {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
    static let fixtures = root.appendingPathComponent("spec/fixtures")

    func testEveryScenarioAgreesWithTheRulebook() throws {
        let paths = try FileManager.default.contentsOfDirectory(at: Self.fixtures, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
        var bad = 0
        for path in paths {
            let failures: [String]
            do {
                failures = try run(path)
            } catch {
                failures = ["\(error)"]
            }
            if failures.isEmpty {
                print("ok   \(path.lastPathComponent)")
            } else {
                bad += 1
                print("FAIL \(path.lastPathComponent)")
                failures.forEach { print("     \($0)") }
            }
        }
        print("\(paths.count - bad) of \(paths.count) scenarios pass")
        XCTAssertEqual(paths.count, 91, "the number of scenarios changed")
        XCTAssertEqual(bad, 0, "\(bad) scenarios failed")
    }

    // MARK: - The delivery

    struct ScenarioError: Error, CustomStringConvertible {
        let description: String
    }

    static func delivery(_ fx: JSON) throws -> JSON {
        var delivery: JSON
        if let name = fx["canvas"]?.string {
            delivery = try JSON(file: fixtures.appendingPathComponent("canvases/\(name).json"))
        } else {
            delivery = fx["canvas"] ?? .null
        }
        let patch = fx["patch"] ?? .object([:])
        if let d = patch["delivery"] {
            delivery.merge(d)
        }
        guard var graph = delivery["runtimeGraph"], graph.object != nil else {
            throw ScenarioError(description: "no runtimeGraph")
        }
        // Edges and inputs are only read when the patch changes them: a
        // delivery refused for its version may have any other shape.
        let edgePatches = patch["edges"]?.object ?? [:]
        let addEdges = patch["addEdges"]?.array ?? []
        if !edgePatches.isEmpty || !addEdges.isEmpty {
            guard var edges = graph["edges"]?.array else {
                throw ScenarioError(description: "no runtimeGraph.edges")
            }
            for (id, change) in edgePatches {
                guard let i = edges.firstIndex(where: { $0["id"]?.string == id }) else {
                    throw ScenarioError(description: "patched edge \(id) missing")
                }
                if change == .null {
                    edges.remove(at: i)
                } else {
                    edges[i].merge(change)
                }
            }
            graph["edges"] = .array(edges + addEdges)
        }
        if graph["inputs"] == nil {
            graph["inputs"] = .array([])
        }
        let addInputs = patch["addInputs"]?.array ?? []
        let defaults = patch["defaults"]?.object ?? [:]
        if !addInputs.isEmpty || !defaults.isEmpty {
            guard var inputs = graph["inputs"]?.array else {
                throw ScenarioError(description: "no runtimeGraph.inputs")
            }
            inputs += addInputs
            for (name, value) in defaults {
                guard let i = inputs.firstIndex(where: { $0["name"]?.string == name }) else {
                    throw ScenarioError(description: "input \(name) missing")
                }
                inputs[i]["default"] = value
            }
            graph["inputs"] = .array(inputs)
        }
        delivery["runtimeGraph"] = graph
        return delivery
    }

    // MARK: - The ideal player

    static func roundMs(_ x: Double) -> UInt64 {
        UInt64(max(x.rounded(.toNearestOrEven), 0))
    }

    final class IdealPlayer {
        let engine: MaskoEngine
        /// Each edge's file length in seconds.
        let files: [String: Double]
        let durations: [String: Double]
        var noEnd: Set<String> = []
        /// How long some edges' first frames take to show, in milliseconds.
        let load: [String: UInt64]
        /// The clip the player has on screen: edge, shown at, rate.
        var clip: (edge: String, shown: UInt64, rate: Double)?
        /// A clip asked for and not yet reported shown: edge, when it shows.
        var showing: (edge: String, at: UInt64)?
        var events: [String] = []
        var preloaded: [String] = []

        init(engine: MaskoEngine, files: [String: Double], durations: [String: Double], load: [String: UInt64]) {
            self.engine = engine
            self.files = files
            self.durations = durations
            self.load = load
        }

        func absorb(_ commands: [EngineCommand]) {
            let now = engine.state().clock
            for c in commands {
                switch c {
                case let .showClip(edge, _, rate):
                    events.append("shown:\(edge)")
                    let at = now + (load[edge] ?? 0)
                    clip = (edge, at, rate)
                    showing = (edge, at)
                case let .showStill(pose):
                    events.append("shown:still:\(pose)")
                    clip = nil
                    showing = nil
                case let .preload(edge):
                    preloaded.append(edge)
                case let .poseEntered(pose):
                    events.append("pose:\(pose)")
                case let .interrupted(edge):
                    events.append("interrupted:\(edge)")
                case let .spent(name, used):
                    events.append(used ? "spent:\(name)" : "spent:\(name):unused")
                case let .error(code, _):
                    events.append("error:\(code)")
                case .playSound, .stopSounds:
                    break
                }
            }
            // A clip that shows at once is reported shown at once.
            if let showing, showing.at == now {
                self.showing = nil
                absorb(engine.clipShown(edge: showing.edge, now: now))
            }
        }

        /// When the player reports the end of its clip, unless a step took
        /// the end signal away.
        func clipEnd() throws -> (edge: String, at: UInt64)? {
            guard let clip, !noEnd.contains(clip.edge) else { return nil }
            guard let length = files[clip.edge] ?? durations[clip.edge] else {
                throw ScenarioError(description: "no duration for \(clip.edge)")
            }
            return (clip.edge, clip.shown + ScenarioTests.roundMs(length / clip.rate * 1000))
        }

        func advance(to until: UInt64) throws {
            for _ in 0..<100_000 {
                if let showing, showing.at <= until, engine.nextWake().map({ showing.at <= $0 }) ?? true {
                    self.showing = nil
                    absorb(engine.clipShown(edge: showing.edge, now: showing.at))
                    continue
                }
                let end = try clipEnd()
                let wake = engine.nextWake()
                let next: UInt64?
                switch (end, wake) {
                case let (e?, w?): next = min(e.at, w)
                case let (e?, nil): next = e.at
                case let (nil, w): next = w
                }
                guard let t = next, t <= until else {
                    absorb(engine.advance(now: until))
                    return
                }
                if let end, end.at <= wake ?? .max {
                    let onScreen = engine.state().clip
                    guard onScreen == end.edge else {
                        throw ScenarioError(
                            description: "the player shows \(end.edge), the engine \(onScreen ?? "nothing")")
                    }
                    absorb(engine.clipEnded(edge: end.edge, now: end.at))
                } else {
                    absorb(engine.advance(now: t))
                }
            }
            throw ScenarioError(description: "the clock did not settle")
        }

        func runToEnd(moving: Bool) throws {
            let state = engine.state()
            guard state.moving == moving else {
                throw ScenarioError(description: "loopEnded or clipEnded on the wrong kind of clip")
            }
            let safety = state.shownAt + state.expectedMs + 1000
            let until: UInt64
            if state.clip?.hasPrefix("still:") == true {
                until = state.shownAt + state.expectedMs
            } else if let end = try clipEnd() {
                until = min(end.at, safety)
            } else {
                until = safety
            }
            try advance(to: until)
        }
    }

    static func value(_ v: JSON) throws -> EngineValue {
        switch v {
        case let .bool(b): return .bool(value: b)
        case .int, .double: return .number(value: v.number!)
        default: throw ScenarioError(description: "not a value: \(v)")
        }
    }

    func act(_ p: IdealPlayer, _ step: JSON) throws {
        let e = p.engine
        let now = e.state().clock
        var commands: [EngineCommand] = []
        // `"instant": true` on a set or fire step (rules.md 5.8).
        let instant = step["instant"]?.bool ?? false
        if step["start"] != nil {
            commands = e.start(now: now)
        } else if let s = step["set"]?.array, s.count == 2, let name = s[0].string {
            switch try Self.value(s[1]) {
            case let .bool(b): commands = e.setBool(name: name, value: b, instant: instant, now: now)
            case let .number(n): commands = e.setNumber(name: name, value: n, instant: instant, now: now)
            }
        } else if let name = step["fire"]?.string {
            commands = e.fire(name: name, instant: instant, now: now)
        } else if let ms = step["advance"]?.number {
            try p.advance(to: now + UInt64(ms))
        } else if step["loopEnded"] != nil {
            try p.runToEnd(moving: false)
        } else if step["clipEnded"] != nil {
            try p.runToEnd(moving: true)
        } else if let edge = step["failClip"]?.string {
            e.markUnplayable(edge: edge)
        } else if let edge = step["noEndSignal"]?.string {
            p.noEnd.insert(edge)
        } else if step["reset"] != nil {
            commands = e.reset(now: now)
        } else if let pose = step["showPose"]?.string {
            commands = e.previewShowPose(pose: pose, now: now)
        } else if let edge = step["playMove"]?.string {
            commands = e.previewPlayMove(edge: edge, now: now)
        } else {
            throw ScenarioError(description: "unknown step \(step)")
        }
        p.absorb(commands)
    }

    static func json(_ v: EngineValue) -> JSON {
        switch v {
        case let .bool(b): return .bool(b)
        case let .number(n): return .double(n)
        }
    }

    func observed(_ p: IdealPlayer, _ want: [String: JSON]) -> [String: JSON] {
        let s = p.engine.state()
        var out: [String: JSON] = [
            "clock": .int(Int64(s.clock)),
            "pose": s.pose.map(JSON.string) ?? .null,
            "clip": s.clip.map(JSON.string) ?? .null,
            "events": .array(p.events.map(JSON.string)),
        ]
        if let clip = s.nextClip {
            let at: JSON = s.nextAtEnd ? .string("end") : .int(Int64(s.nextAt ?? 0))
            out["next"] = .object(["clip": .string(clip), "at": at])
        } else {
            out["next"] = .null
        }
        if let names = want["values"]?.object {
            out["values"] = .object(
                Dictionary(uniqueKeysWithValues: names.keys.map { ($0, Self.json(p.engine.value(name: $0))) }))
        }
        if want["fired"] != nil {
            out["fired"] = .array(p.engine.pendingFires().map(JSON.string))
        }
        if want["preloaded"] != nil {
            out["preloaded"] = .array(p.preloaded.map(JSON.string))
        }
        return out
    }

    func run(_ path: URL) throws -> [String] {
        try run(fx: JSON(file: path))
    }

    /// The runner itself must catch a wrong trace.
    func testAWrongExpectationFails() throws {
        var fx = try JSON(file: Self.fixtures.appendingPathComponent("fire-reaction-pose-plays-one-loop.json"))
        XCTAssertEqual(try run(fx: fx), [])
        var steps = fx["steps"]!.array!
        steps[2]["expect"]?["clock"] = .int(4001)
        fx["steps"] = .array(steps)
        XCTAssertEqual(try run(fx: fx), ["step 3 clock: got 4000, expected 4001"])
    }

    func run(fx: JSON) throws -> [String] {
        let delivery = try Self.delivery(fx)
        let wantRefusal = fx["expectLoad"]?["refused"]?.string
        let engine: MaskoEngine
        do {
            engine = try MaskoEngine(delivery: delivery.text)
        } catch let LoadFailure.Refused(code, _) {
            return wantRefusal == code ? [] : ["load refused with \(code), expected \(wantRefusal ?? "none")"]
        }
        if let code = wantRefusal {
            return ["loaded, expected refusal \(code)"]
        }
        engine.useRolls(rolls: (fx["random"]?.array ?? []).compactMap(\.number))
        let files = (fx["files"]?.object ?? [:]).compactMapValues(\.number)
        let load = (fx["loadMs"]?.object ?? [:]).compactMapValues { $0.number.map { UInt64($0) } }
        var durations: [String: Double] = [:]
        for edge in delivery["runtimeGraph"]?["edges"]?.array ?? [] {
            if let id = edge["id"]?.string, let d = edge["duration"]?.number {
                durations[id] = d
            }
        }
        let player = IdealPlayer(engine: engine, files: files, durations: durations, load: load)
        var failures: [String] = []
        for (i, step) in (fx["steps"]?.array ?? []).enumerated() {
            player.events = []
            player.preloaded = []
            try act(player, step)
            let want = step["expect"]?.object ?? [:]
            let got = observed(player, want)
            for (key, value) in want.sorted(by: { $0.key < $1.key }) {
                let g = got[key] ?? .null
                if !g.same(value) {
                    failures.append("step \(i + 1) \(key): got \(g), expected \(value)")
                }
            }
        }
        return failures
    }
}
