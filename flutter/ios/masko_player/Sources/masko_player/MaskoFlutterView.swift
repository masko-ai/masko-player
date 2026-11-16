import Flutter
import UIKit

#if canImport(MaskoPlayer)
import MaskoPlayer
#endif

/// Shows one `MaskoPlayer` in a `MaskoPlayerView` for a Flutter `MaskoView`.
/// A new canvas URL or app key, delivery or clip, or `getDelivery` given or
/// taken away, loads a new player; the view going away destroys it. Calls come on the view's own
/// channel, on the main thread, in the order Dart made them; events go back
/// on it. A `getDelivery` call waits for Dart without holding the view, and
/// fails when the view goes away, so a view Flutter dropped is freed even
/// when Dart never answers.
@MainActor
final class MaskoFlutterView: NSObject, FlutterPlatformView {
    private let channel: FlutterMethodChannel
    private let mascot = MaskoPlayerView()

    private var player: MaskoPlayer?
    /// A canvas's URL, played in place of `delivery`: the player asks it for
    /// the latest release and keeps the files.
    private var url: String?
    /// The app key that reads `url`, unless the URL carries it as `?key=`.
    private var appKey: String?
    private var delivery: String?
    /// A lone clip's HEVC file, played in place of `delivery`.
    private var clip: String?
    private var autoPlay: Bool?
    private var sound = true
    private var refreshes = false

    /// What the player was loaded with: the delivery's JSON, `clip:` and the
    /// file, or `url:` with the URL and the key.
    private var loaded: (source: String, refreshes: Bool)?
    /// `getDelivery` calls waiting for Dart's answer.
    private var requests: [Int: CheckedContinuation<Data, Error>] = [:]
    private var lastRequest = 0

    /// `arguments` are the widget's props, its channel's number and the
    /// calls the app made before the view existed, which reach the player
    /// before it starts.
    init(arguments: [String: Any], messenger: FlutterBinaryMessenger) {
        let id = arguments["channel"] as? Int ?? 0
        channel = FlutterMethodChannel(name: "ai.masko.player/view_\(id)", binaryMessenger: messenger)
        super.init()
        channel.setMethodCallHandler { [weak self] call, result in
            MainActor.assumeIsolated {
                guard let self else { return result(nil) }
                self.handle(call, result: result)
            }
        }
        apply(arguments)
        for call in arguments["calls"] as? [[String: Any]] ?? [] {
            _ = run(call["method"] as? String ?? "", call["arguments"] as? [String: Any] ?? [:])
        }
    }

    deinit {
        for request in requests.values {
            request.resume(throwing: DeliveryUnavailable())
        }
        channel.setMethodCallHandler(nil)
        let player = self.player
        if Thread.isMainThread {
            MainActor.assumeIsolated { player?.destroy() }
        } else {
            Task { @MainActor in player?.destroy() }
        }
    }

    nonisolated func view() -> UIView {
        MainActor.assumeIsolated { mascot }
    }

    private func handle(_ call: FlutterMethodCall, result: FlutterResult) {
        if call.method == "position" {
            let position = player?.position()
            result(["clip": position?.clip.map { $0 as Any } ?? NSNull(), "time": position?.time ?? 0])
            return
        }
        result(run(call.method, call.arguments as? [String: Any] ?? [:]) ? nil : FlutterMethodNotImplemented)
    }

    /// One call of the app's. False for a call this view does not know.
    private func run(_ method: String, _ arguments: [String: Any]) -> Bool {
        let name = arguments["name"] as? String ?? ""
        let instant = arguments["instant"] as? Bool ?? false
        switch method {
        case "update": apply(arguments)
        case "setBool": player?.set(name, arguments["value"] as? Bool ?? false, instant: instant)
        case "setNumber": player?.set(name, (arguments["value"] as? NSNumber)?.doubleValue ?? 0, instant: instant)
        case "fire": player?.fire(name, instant: instant)
        case "reset": player?.reset()
        case "play": player?.play()
        case "pause": player?.pause()
        case "showPose": player?.preview.showPose(arguments["id"] as? String ?? "")
        case "playMove": player?.preview.playMove(arguments["id"] as? String ?? "")
        // New links for the release that is playing: the player swaps them in.
        case "updateDelivery": _ = player?.updateDelivery(Data((arguments["delivery"] as? String ?? "").utf8))
        default: return false
        }
        return true
    }

    /// The widget's props: loads what they name when it changed.
    private func apply(_ props: [String: Any]) {
        url = props["url"] as? String
        appKey = props["appKey"] as? String
        delivery = props["delivery"] as? String
        clip = props["clip"] as? String
        autoPlay = props["autoPlay"] as? Bool
        sound = props["sound"] as? Bool ?? true
        refreshes = props["refreshes"] as? Bool ?? false
        if let url, !url.isEmpty {
            let source = "url:" + url + " " + (appKey ?? "")
            if loaded?.source != source { loadCanvas(url, source: source) }
        } else if let clip, let url = clip.hasPrefix("/") ? URL(fileURLWithPath: clip) : URL(string: clip) {
            if loaded?.source != "clip:" + clip { loadClip(url, source: "clip:" + clip) }
        } else if let delivery, !delivery.isEmpty {
            if loaded?.source != delivery || loaded?.refreshes != refreshes { load(delivery) }
        } else {
            unload()
        }
        player?.sound = sound
    }

    private func load(_ json: String) {
        unload()
        var getDelivery: (@MainActor () async throws -> Data)?
        if refreshes {
            // Only the continuation waits for the answer, never the view.
            getDelivery = { [weak self] in
                try await withCheckedThrowingContinuation { continuation in
                    MainActor.assumeIsolated {
                        guard let self else { return continuation.resume(throwing: DeliveryUnavailable()) }
                        self.requestDelivery(continuation)
                    }
                }
            }
        }
        show(MaskoPlayer(delivery: Data(json.utf8), autoPlay: autoPlay, getDelivery: getDelivery))
        loaded = (json, refreshes)
    }

    /// Plays a canvas from its URL; the player asks for its links itself, so
    /// `getDelivery` is not used.
    private func loadCanvas(_ link: String, source: String) {
        unload()
        guard let url = URL(string: link) else {
            channel.invokeMethod("error", arguments: ["code": "refresh_failed", "detail": "not a URL: \(link)"])
            return
        }
        show(MaskoPlayer(url: url, appKey: appKey?.isEmpty == false ? appKey : nil, autoPlay: autoPlay))
        loaded = (source, refreshes)
    }

    private func loadClip(_ url: URL, source: String) {
        unload()
        show(MaskoPlayer(clip: url, autoPlay: autoPlay))
        loaded = (source, refreshes)
    }

    private func show(_ player: MaskoPlayer) {
        player.sound = sound
        player.onEvent = { [weak self] event in self?.send(event) }
        mascot.player = player
        self.player = player
    }

    private func unload() {
        failRequests()
        player?.destroy()
        player = nil
        mascot.player = nil
        loaded = nil
    }

    /// Asks Dart's `getDelivery`; its answer resumes the continuation.
    private func requestDelivery(_ continuation: CheckedContinuation<Data, Error>) {
        lastRequest += 1
        let request = lastRequest
        requests[request] = continuation
        channel.invokeMethod("getDelivery", arguments: nil) { [weak self] answer in
            MainActor.assumeIsolated {
                guard let continuation = self?.requests.removeValue(forKey: request) else { return }
                if let json = answer as? String {
                    continuation.resume(returning: Data(json.utf8))
                } else {
                    continuation.resume(throwing: DeliveryUnavailable())
                }
            }
        }
    }

    /// Ends every `getDelivery` call still waiting for Dart.
    private func failRequests() {
        let waiting = requests.values
        requests = [:]
        for request in waiting {
            request.resume(throwing: DeliveryUnavailable())
        }
    }

    private func send(_ event: MaskoPlayer.Event) {
        switch event {
        case let .pose(pose):
            channel.invokeMethod("pose", arguments: ["pose": pose])
        case let .clip(clip, kind, rate):
            channel.invokeMethod("clip", arguments: ["clip": clip, "kind": kind.rawValue, "rate": rate])
        case let .reactionDone(name, used):
            channel.invokeMethod("reactionDone", arguments: ["name": name, "used": used])
        case let .movement(edge, movement, rate, seconds):
            channel.invokeMethod(
                "movement", arguments: ["edge": edge, "movement": movement.plain, "rate": rate, "seconds": seconds])
        case let .error(code, detail):
            var payload: [String: Any] = ["code": code]
            payload["detail"] = detail
            channel.invokeMethod("error", arguments: payload)
        }
    }
}

/// `getDelivery` gave nothing: the player reports `refresh_failed`.
private struct DeliveryUnavailable: Error {}

private extension MaskoJSON {
    /// The value as Foundation types, for Dart.
    var plain: Any {
        switch self {
        case .null: return NSNull()
        case let .bool(value): return value
        case let .number(value): return value
        case let .string(value): return value
        case let .array(values): return values.map(\.plain)
        case let .object(values): return values.mapValues(\.plain)
        }
    }
}
