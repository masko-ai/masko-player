import ExpoModulesCore
import UIKit

/// Shows one `MaskoPlayer` in a `MaskoPlayerView`. A new delivery, canvas URL,
/// app key or clip, or `getDelivery` given or taken away, loads a new player; the view going away
/// destroys it. A `getDelivery` call waits for JavaScript without holding the
/// view, and fails when the view leaves its window, so an unmounted view is
/// freed even when JavaScript never answers.
final class MaskoNativeView: ExpoView {
  let onPose = EventDispatcher()
  let onClip = EventDispatcher()
  let onReactionDone = EventDispatcher()
  let onMovement = EventDispatcher()
  let onError = EventDispatcher()
  let onDeliveryRequest = EventDispatcher()

  var delivery: String?
  /// A lone clip's HEVC file, played in place of `delivery`.
  var clip: String?
  /// A canvas's delivery URL, played in place of `delivery`: the player
  /// fetches the latest release itself.
  var url: String?
  /// The app key for `url`, when the URL does not carry one.
  var appKey: String?
  var autoPlay: Bool?
  var sound = true
  var refreshes = false

  private(set) var player: MaskoPlayer?
  private let mascot = MaskoPlayerView()
  /// What the player was loaded with: the delivery's JSON, `clip:` and the
  /// file, or `url:` with the URL and the app key.
  private var loaded: (source: String, refreshes: Bool)?
  /// `getDelivery` calls waiting for JavaScript's answer.
  private var requests: [Int: CheckedContinuation<Data, Error>] = [:]
  private var lastRequest = 0

  required init(appContext: AppContext? = nil) {
    super.init(appContext: appContext)
    addSubview(mascot)
  }

  deinit {
    for request in requests.values {
      request.resume(throwing: DeliveryUnavailable())
    }
    let player = self.player
    if Thread.isMainThread {
      MainActor.assumeIsolated { player?.destroy() }
    } else {
      Task { @MainActor in player?.destroy() }
    }
  }

  override func layoutSubviews() {
    super.layoutSubviews()
    mascot.frame = bounds
  }

  /// Out of its window, as when React Native unmounts it, JavaScript may
  /// never answer: the waiting calls fail, and the player reports
  /// `refresh_failed` and asks again 30 seconds later.
  override func didMoveToWindow() {
    super.didMoveToWindow()
    if window == nil { failRequests() }
  }

  func propsChanged() {
    if let clip, let url = URL(string: clip) {
      if loaded?.source != "clip:" + clip { loadClip(url) }
    } else if let url, !url.isEmpty {
      guard let canvas = URL(string: url) else { return unload() }
      let key = appKey.flatMap { $0.isEmpty ? nil : $0 }
      let source = "url:" + url + "\n" + (key ?? "")
      if loaded?.source != source { loadCanvas(canvas, appKey: key, source: source) }
    } else {
      guard let delivery, !delivery.isEmpty else { return unload() }
      if loaded?.source != delivery || loaded?.refreshes != refreshes {
        load(delivery)
      }
    }
    player?.sound = sound
  }

  func position() -> [String: Any] {
    let position = player?.position()
    return ["clip": position?.clip.map { $0 as Any } ?? NSNull(), "time": position?.time ?? 0]
  }

  /// New links for the release that is playing: the player swaps them in.
  func updateDelivery(_ json: String) {
    player?.updateDelivery(Data(json.utf8))
  }

  func answerDelivery(_ request: Int, json: String?) {
    guard let continuation = requests.removeValue(forKey: request) else { return }
    if let json {
      continuation.resume(returning: Data(json.utf8))
    } else {
      continuation.resume(throwing: DeliveryUnavailable())
    }
  }

  private func load(_ json: String) {
    unload()
    var getDelivery: (@MainActor () async throws -> Data)?
    if refreshes {
      // Only the continuation waits for the answer, never the view.
      getDelivery = { [weak self] in
        try await withCheckedThrowingContinuation { continuation in
          guard let self else { return continuation.resume(throwing: DeliveryUnavailable()) }
          self.requestDelivery(continuation)
        }
      }
    }
    let player = MaskoPlayer(delivery: Data(json.utf8), autoPlay: autoPlay, getDelivery: getDelivery)
    show(player)
    loaded = (json, refreshes)
  }

  private func loadClip(_ url: URL) {
    unload()
    show(MaskoPlayer(clip: url, autoPlay: autoPlay))
    loaded = ("clip:" + url.absoluteString, refreshes)
  }

  /// A canvas from its URL: the player fetches its deliveries, so
  /// `getDelivery` plays no part.
  private func loadCanvas(_ url: URL, appKey: String?, source: String) {
    unload()
    show(MaskoPlayer(url: url, appKey: appKey, autoPlay: autoPlay))
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

  /// Asks JavaScript's `getDelivery`; `answerDelivery` resumes the
  /// continuation.
  private func requestDelivery(_ continuation: CheckedContinuation<Data, Error>) {
    lastRequest += 1
    requests[lastRequest] = continuation
    onDeliveryRequest(["request": lastRequest])
  }

  /// Ends every `getDelivery` call still waiting for JavaScript.
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
      onPose(["pose": pose])
    case let .clip(clip, kind, rate):
      onClip(["clip": clip, "kind": kind.rawValue, "rate": rate])
    case let .reactionDone(name, used):
      onReactionDone(["name": name, "used": used])
    case let .movement(edge, movement, rate, seconds):
      onMovement(["edge": edge, "movement": movement.plain, "rate": rate, "seconds": seconds])
    case let .error(code, detail):
      var payload: [String: Any] = ["code": code]
      payload["detail"] = detail
      onError(payload)
    }
  }
}

/// `getDelivery` gave nothing: the player reports `refresh_failed`.
private struct DeliveryUnavailable: Error {}

private extension MaskoJSON {
  /// The value as Foundation types, for JavaScript.
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
