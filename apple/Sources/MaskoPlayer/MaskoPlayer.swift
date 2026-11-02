import CoreGraphics
import Foundation

/// Plays one Masko mascot release. The engine decides which clip plays and
/// when (`spec/rules.md`); this player keeps the play-time clock, shows the
/// clips the engine names and reports when they show and end
/// (`spec/player.md`).
///
/// ```swift
/// let player = MaskoPlayer(delivery: json)
/// // or a canvas read from its URL with an app key, kept on the device:
/// // MaskoPlayer(url: URL(string: "https://api.masko.ai/v1/canvases/<id>/delivery")!, appKey: "masko_app_...")
/// player.set("working", true)
/// player.fire("celebrate")
/// player.onEvent = { event in print(event) }
/// MaskoView(player: player) // or MaskoPlayerView(player: player)
/// ```
@MainActor
public final class MaskoPlayer {
    /// The `formats` to ask the release delivery for (player.md, "Asking for
    /// AV1"): `av1`, which this player plays on every device, with the
    /// device's AV1 decoder or with dav1d.
    nonisolated public static let formats = "av1"

    /// What the app hears (spec/player.md, "What the app sees").
    public enum Event: Equatable, Sendable {
        /// The mascot is on this pose now.
        case pose(pose: String)
        /// This clip is on screen now: an edge id, or `still:<pose>` for a
        /// pose's still, with its kind and its playback rate.
        case clip(clip: String, kind: ClipKind, rate: Double)
        /// A fire is done; `used` when a start used it (rules.md 3.3).
        case reactionDone(name: String, used: Bool)
        /// A move's clip is on screen: move the mascot as the edge's
        /// `movement` says, over `seconds` (the edge's duration at this
        /// rate). The player never moves itself.
        case movement(edge: String, movement: MaskoJSON, rate: Double, seconds: Double)
        /// Something went wrong, with a code from the rulebook or the player
        /// and what it is about: the clip of a clip error, the name of a
        /// refused value, the pose of a pose error.
        case error(code: String, detail: String?)
    }

    /// What a `clip` event shows: a pose clip, a move, or a pose's still.
    public enum ClipKind: String, Equatable, Sendable {
        case pose
        case move
        case still
    }

    /// The clip on screen and how far into it the picture is.
    public struct Position: Equatable, Sendable {
        /// The edge on screen, `still:<pose>`, or nil before anything shows.
        public let clip: String?
        /// Clip time in seconds; 0 for a still or nothing.
        public let time: Double
    }

    /// Calls for editors and tools only (rules.md 13).
    @MainActor
    public struct Preview {
        fileprivate let player: MaskoPlayer

        /// Cuts to this pose at once.
        public func showPose(_ id: String) {
            player.call(beforeStart: .afterStart) { $0.previewShowPose(pose: id, now: $1) }
        }

        /// Plays this move from the current pose at once.
        public func playMove(_ id: String) {
            player.call(beforeStart: .afterStart) { $0.previewPlayMove(edge: id, now: $1) }
        }
    }

    /// Hears every event, in order.
    public var onEvent: ((Event) -> Void)?

    /// Lets clips' own voices (`hasAudio`) be heard. On by default; false
    /// mutes them. Edge sounds play either way (rules.md 9.6).
    public var sound = true {
        didSet { muteVoices() }
    }

    /// Returns a fresh delivery of the same release. The player calls it
    /// before `deliveryAccess.signedUrlExpiresAt` and swaps the links only.
    public var getDelivery: (@MainActor () async throws -> Data)? {
        didSet { scheduleRefresh() }
    }

    public var preview: Preview { Preview(player: self) }

    /// The mascot's size on screen in pixels (its longer side), set by the
    /// view, for size variants. A new size picks the file of the next clip,
    /// and of the pose clip on screen at its next loop.
    var displayPixels: Double? {
        didSet {
            if displayPixels != oldValue { prepareNext() }
        }
    }

    /// The views showing this player (`MaskoPlayerView`).
    var hosts = StageHosts()

    let stage: Stage
    private let loader: MediaLoader
    private let sounds: SoundOutput
    private let host: PlayerHost
    private var json: String
    /// A lone clip whose delivery waits for its file's length (player.md,
    /// "One clip").
    private var clip: URL?
    private var delivery: Delivery?
    private(set) var engine: MaskoEngine?
    private var refusal: (code: String, detail: String?)?

    private var clock = PlayClock()
    private var autoPlay: Bool
    private var booted = false
    /// The first frame is on screen, or cannot be shown.
    private var firstFrameReady = false
    private var started = false
    /// The app called `pause()`.
    private var appPaused = false
    /// The view is off screen or the app is in the background.
    private(set) var hostPaused = false
    private var paused: Bool { appPaused || hostPaused }
    /// Play time, clips, sounds and timers are stopped.
    private var stopped = false
    private var destroyed = false

    private typealias Call = (MaskoEngine, UInt64) -> [EngineCommand]
    /// The app's calls before the start, replayed to find the first frame.
    private var beforeStart: [Call] = []
    /// Preview calls made before the start, sent right after it.
    private var afterStart: [Call] = []
    /// Calls made while paused, sent when play resumes (rules.md 11.2).
    private var held: [Call] = []
    /// Calls waiting their turn: commands run in order, one call at a time.
    private var queue: [Call] = []
    private var draining = false
    private var wake: Cancellation?
    private var refreshTimer: Cancellation?

    /// A canvas read from its URL with an app key (player.md, "A canvas from
    /// its URL").
    private var release: KeptRelease?
    /// A newer release whose files are all kept, waiting for the initial
    /// pose's loop to start again.
    private var waiting: Data?
    /// The restart of the initial pose's loop the waiting release takes over
    /// from, once the commands running now are done.
    private var takeOver: (edge: String, rate: Double)?
    private var checkTimer: Cancellation?
    /// The check running now, for tests.
    private(set) var checking: Task<Void, Never>?
    /// How long until a failed first check asks again; it doubles up to
    /// `checkEvery`.
    private var retry = refreshRetry
    /// `autoPlay` came from the app (the initializer, `play` or `pause`);
    /// otherwise the graph's applies once a delivery is in.
    private var autoPlayChosen = true
    /// The values the app set, carried over to a newer release.
    private enum Value {
        case bool(Bool)
        case number(Double)
    }
    private var values: [String: Value] = [:]

    /// A clip or still asked for by the engine (or the first frame shown
    /// before the start, a poster).
    private struct Showing {
        var edge: String
        var move: Bool
        var rate: Double
        var token: Int
        var poster: Bool
        var ended = false
        /// The file loaded for it (`Delivery.file`'s key); none for a still.
        var key: String?
    }

    /// What each slot shows, or is loading.
    private var shows: [Showing?] = [nil, nil]
    /// The slot on screen; none while a still or nothing is shown.
    private var visible: Int?
    private var still: Showing?
    private var token = 0
    /// The latest showing asked for. Only it may swap in, be reported shown,
    /// end or fail.
    private var current = 0
    /// Edges the engine was told cannot play.
    private var failed: Set<String> = []
    private var soundToken = 0
    /// Sounds still downloading, and whether each loops.
    private var pendingSounds: [Int: Bool] = [:]
    /// The pose clip the engine last asked to preload: the main loop of the
    /// target of the move it shows, decoded in the free slot during the move.
    private var loopAhead: String?

    /// How long before signed links expire the player asks for fresh ones;
    /// halfway there when they expire sooner than twice this.
    static let refreshLead: TimeInterval = 5 * 60
    /// How long after a failed refresh it asks again.
    static let refreshRetry: TimeInterval = 30
    /// How often a canvas read from its URL asks for a newer release.
    static let checkEvery: TimeInterval = 60 * 60

    /// Loads a release delivery (the `delivery` object of
    /// `GET /v1/canvases/{canvasId}/releases/{releaseId}/delivery`).
    ///
    /// On the next turn of the main run loop the player loads the first
    /// frame, so values set right after this call choose the first pose
    /// (rules.md 11.1). The mascot starts, at play time 0, once that frame is
    /// on screen; with `autoPlay` off (here or in the graph) it waits there
    /// for `play()`. A delivery the engine refuses plays nothing and reports
    /// an `error` event with the reason.
    public convenience init(
        delivery: Data,
        autoPlay: Bool? = nil,
        getDelivery: (@MainActor () async throws -> Data)? = nil
    ) {
        self.init(
            delivery: delivery, autoPlay: autoPlay, stage: AVStage(), loader: FileCache(),
            sounds: AVSounds(), host: .real)
        engine?.seed(seed: UInt64.random(in: 1...UInt64.max))
        self.getDelivery = getDelivery
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.boot() }
        }
    }

    init(delivery: Data, autoPlay: Bool?, stage: Stage, loader: MediaLoader, sounds: SoundOutput, host: PlayerHost) {
        self.stage = stage
        self.loader = loader
        self.sounds = sounds
        self.host = host
        json = ""
        self.autoPlay = autoPlay ?? true
        autoPlayChosen = autoPlay != nil
        load(delivery)
        wireSlots()
    }

    /// Plays a canvas's latest release from its URL,
    /// `https://api.masko.ai/v1/canvases/{canvasId}/delivery`, read with an
    /// app key of its mascot (player.md, "A canvas from its URL"). Every file
    /// is downloaded once and kept, so the mascot also plays offline. The
    /// player asks for a newer release at once and every hour, keeps all its
    /// files, then switches to it when the mascot's first pose starts its
    /// loop again. Before the first delivery is in, values set choose the
    /// first pose and fires are dropped.
    public convenience init(url: URL, appKey: String, autoPlay: Bool? = nil) {
        let release = KeptRelease(url: url, appKey: appKey)
        self.init(
            release: release, autoPlay: autoPlay, stage: AVStage(), loader: FileCache(keptIn: release.files),
            sounds: AVSounds(), host: .real)
        engine?.seed(seed: UInt64.random(in: 1...UInt64.max))
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.boot() }
        }
    }

    init(
        release: KeptRelease, autoPlay: Bool?, stage: Stage, loader: MediaLoader, sounds: SoundOutput,
        host: PlayerHost
    ) {
        self.stage = stage
        self.loader = loader
        self.sounds = sounds
        self.host = host
        json = ""
        self.release = release
        self.autoPlay = autoPlay ?? true
        autoPlayChosen = autoPlay != nil
        wireSlots()
        // Before anything plays: files of older releases go.
        release.removeUnused()
        if let kept = release.kept() {
            load(kept)
        }
    }

    /// Reads a release delivery for the engine and the player. A delivery the
    /// engine refuses plays nothing, with the engine's own reason, as on every
    /// platform: `unreadable` for text that is not JSON.
    private func load(_ data: Data) {
        json = String(decoding: data, as: UTF8.self)
        do {
            engine = try MaskoEngine(delivery: json)
            delivery = try Delivery(json: data)
            refusal = nil
        } catch let LoadFailure.Refused(code, detail) {
            engine = nil
            delivery = nil
            refusal = (code, detail)
        } catch {
            engine = nil
            delivery = nil
            refusal = ("unreadable", String(describing: error))
        }
        if !autoPlayChosen {
            autoPlay = delivery?.autoPlay ?? true
        }
    }

    /// A lone clip reading its length, or a canvas waiting for its first
    /// delivery.
    private var loading: Bool {
        clip != nil || (release != nil && engine == nil && refusal == nil)
    }

    /// A player for one animation on its own: its HEVC file, with
    /// transparency, looped as one pose. The player reads the file's length
    /// first; a file it cannot read plays nothing and reports an `error`
    /// event `clip_failed`.
    public convenience init(clip: URL, autoPlay: Bool? = nil) {
        self.init(
            clip: clip, autoPlay: autoPlay, stage: AVStage(), loader: FileCache(), sounds: AVSounds(),
            host: .real)
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.boot() }
        }
    }

    init(clip: URL, autoPlay: Bool?, stage: Stage, loader: MediaLoader, sounds: SoundOutput, host: PlayerHost) {
        self.stage = stage
        self.loader = loader
        self.sounds = sounds
        self.host = host
        json = ""
        self.clip = clip
        self.autoPlay = autoPlay ?? true
        wireSlots()
    }

    private func wireSlots() {
        for (i, slot) in stage.slots.enumerated() {
            slot.onEnd = { [weak self] in self?.slotEnded(i) }
            slot.onFail = { [weak self] in self?.slotFailed(i) }
        }
    }

    /// Loads the first frame; the mascot starts once it shows, with
    /// `autoPlay`.
    func boot() {
        guard !booted, !destroyed else { return }
        booted = true
        if let clip {
            readClip(clip)
            return
        }
        if release != nil {
            check()
            // The first delivery plays once it is in.
            if engine == nil, refusal == nil { return }
        }
        if let refusal {
            emit(.error(code: refusal.code, detail: refusal.detail))
            return
        }
        scheduleRefresh()
        showPoster()
    }

    /// A lone clip's delivery, once its file's length is known; then the
    /// first frame loads as for a release.
    private func readClip(_ url: URL) {
        loader.clipFile(url, key: "clip") { [weak self] file in
            guard let self, !self.destroyed else { return }
            self.clip = nil
            guard let file else {
                self.emit(.error(code: "clip_failed", detail: "cannot read the length of \(url.absoluteString)"))
                return
            }
            let data = Delivery.clip(file: url, seconds: file.seconds, voiced: file.voiced)
            self.json = String(decoding: data, as: UTF8.self)
            self.engine = try? MaskoEngine(delivery: self.json)
            self.engine?.seed(seed: UInt64.random(in: 1...UInt64.max))
            self.delivery = try? Delivery(json: data)
            self.showPoster()
        }
    }

    // MARK: - What the app calls

    /// Holds a boolean until the next `set` of that name (rules.md 3.2).
    /// With `instant`, what it starts happens now: an Any State start goes
    /// straight into its target pose, skipping every transition clip and
    /// route, and a move plays its own clip (5.8).
    public func set(_ name: String, _ value: Bool, instant: Bool = false) {
        hold(name, .bool(value))
        call { $0.setBool(name: name, value: value, instant: instant, now: $1) }
    }

    /// Holds a number until the next `set` of that name (rules.md 3.2),
    /// with `instant` as for a boolean.
    public func set(_ name: String, _ value: Double, instant: Bool = false) {
        hold(name, .number(value))
        call { $0.setNumber(name: name, value: value, instant: instant, now: $1) }
    }

    public func set(_ name: String, _ value: Int, instant: Bool = false) {
        set(name, Double(value), instant: instant)
    }

    /// Makes the name read true until a clip uses it (rules.md 3.3), with
    /// `instant` as for `set`.
    public func fire(_ name: String, instant: Bool = false) {
        call { $0.fire(name: name, instant: instant, now: $1) }
    }

    /// Values back to their defaults, and the mascot starts again
    /// (rules.md 11.3).
    public func reset() {
        guard !destroyed, engine != nil else { return }
        values = [:]
        if !started {
            // Nothing has played: forget the values set so far.
            engine = try? MaskoEngine(delivery: json)
            engine?.seed(seed: UInt64.random(in: 1...UInt64.max))
            beforeStart = []
            if booted { showPoster() }
            return
        }
        call { $0.reset(now: $1) }
    }

    /// Starts the mascot (once its first frame shows), or resumes it exactly
    /// where it paused.
    public func play() {
        // A lone clip still reading its length, or a canvas waiting for its
        // first delivery, starts once it can.
        if loading, !destroyed {
            autoPlay = true
            autoPlayChosen = true
            return
        }
        guard !destroyed, engine != nil else { return }
        if !started {
            autoPlay = true
            autoPlayChosen = true
            if booted && firstFrameReady { start() }
            return
        }
        appPaused = false
        applyPause()
    }

    /// Stops play time, clips, sounds and timers (rules.md 11.2). Calls made
    /// while paused are decided on when play resumes. Before the start, it
    /// keeps the mascot on its first frame until `play()`.
    public func pause() {
        guard !destroyed else { return }
        if !started {
            autoPlay = false
            autoPlayChosen = true
            return
        }
        appPaused = true
        applyPause()
    }

    /// The view is on screen and the app in the foreground, or not: while
    /// not, the player pauses itself.
    func setHostActive(_ active: Bool) {
        hostPaused = !active
        applyPause()
    }

    /// Stops or runs play time, clips, sounds and timers to match the pause
    /// (rules.md 11.2). Calls held while paused are sent on resume.
    private func applyPause() {
        guard started, !destroyed, stopped != paused else { return }
        stopped = paused
        if stopped {
            clock.stop(at: host.uptime())
            wake?.cancel()
            wake = nil
            if let v = visible {
                stage.slots[v].pause()
            }
            sounds.pause()
            return
        }
        clock.run(at: host.uptime())
        if let v = visible, let s = shows[v], !s.ended {
            stage.slots[v].play(rate: s.rate)
        }
        sounds.resume()
        let calls = held
        held = []
        calls.forEach(send)
        scheduleWake()
    }

    /// The clip on screen and its clip time; `clip` is nil before anything
    /// shows.
    public func position() -> Position {
        if let v = visible, let s = shows[v] {
            return Position(clip: s.edge, time: stage.slots[v].clipTime)
        }
        return Position(clip: still?.edge, time: 0)
    }

    /// Takes a fresh delivery of the same release, as `getDelivery` gives
    /// one: only its links are used, the engine keeps running, files already
    /// fetched keep playing and clips that failed are tried again. Another
    /// release is refused with `error` `refresh_mismatch`, a delivery that
    /// cannot be read with `refresh_failed`.
    @discardableResult
    public func updateDelivery(_ json: Data) -> Bool {
        guard !destroyed, let current = delivery else { return false }
        let fresh: Delivery
        do {
            fresh = try Delivery(json: json)
        } catch {
            emit(.error(code: "refresh_failed", detail: String(describing: error)))
            return false
        }
        guard current.sameRelease(as: fresh) else {
            emit(.error(code: "refresh_mismatch", detail: "getDelivery returned another release"))
            return false
        }
        delivery?.takeLinks(from: fresh)
        scheduleRefresh()
        // Clips that failed on an expired link may load now.
        let retried = failed
        retried.forEach(preload)
        return true
    }

    /// Stops everything and lets go of every file. The player does nothing
    /// afterwards.
    public func destroy() {
        guard !destroyed else { return }
        destroyed = true
        // Nothing still loading may show or play afterwards.
        token += 1
        current = token
        pendingSounds = [:]
        wake?.cancel()
        refreshTimer?.cancel()
        checkTimer?.cancel()
        waiting = nil
        sounds.stop(onlyLooping: false)
        (stage as? AVStage)?.unload()
        (loader as? FileCache)?.clear()
        engine = nil
        onEvent = nil
    }

    // MARK: - Calls into the engine

    private enum BeforeStart {
        case now
        case afterStart
    }

    /// Play time now, in whole milliseconds.
    var now: UInt64 {
        clock.milliseconds(at: host.uptime())
    }

    private func call(beforeStart when: BeforeStart = .now, _ c: @escaping Call) {
        guard !destroyed, engine != nil else { return }
        if !started {
            switch when {
            case .now:
                // Before the start, at play time 0 (rules.md 11.1).
                beforeStart.append(c)
                send(c)
                if booted { showPoster() }
            case .afterStart:
                afterStart.append(c)
            }
            return
        }
        if paused {
            held.append(c)
            return
        }
        send(c)
    }

    /// Calls the engine, runs the commands it returns in order, then
    /// schedules the next wake. A call made while commands run (a clip that
    /// fails at once, an app reacting to an event) waits its turn.
    private func send(_ c: @escaping Call) {
        queue.append(c)
        guard !draining else { return }
        draining = true
        while !queue.isEmpty, let engine {
            let next = queue.removeFirst()
            for command in next(engine, now) {
                execute(command)
            }
            if let restart = takeOver {
                takeOver = nil
                if !switchRelease() { show(restart.edge, move: false, rate: restart.rate) }
            }
        }
        draining = false
        prepareNext()
        scheduleWake()
    }

    private func scheduleWake() {
        wake?.cancel()
        wake = nil
        guard started, !paused, !destroyed, let at = engine?.nextWake() else { return }
        let ahead = Double(at) / 1000 - clock.seconds(at: host.uptime())
        // Half a millisecond late, so the whole-millisecond clock has reached it.
        wake = host.after(max(ahead, 0) + 0.0005) { [weak self] in
            guard let self else { return }
            self.wake = nil
            self.send { $0.advance(now: $1) }
        }
    }

    private func execute(_ command: EngineCommand) {
        guard !destroyed else { return }
        switch command {
        case let .showClip(edge, kind, rate):
            if waiting != nil, kind == .pose, restartsInitialLoop(edge) {
                // A newer release takes over here, once these commands are done.
                takeOver = (edge, rate)
                return
            }
            show(edge, move: kind == .move, rate: rate)
        case let .showStill(pose):
            showStill(pose)
        case let .preload(edge):
            preload(edge)
        case let .playSound(edge):
            playSound(edge)
        case let .stopSounds(onlyLooping):
            pendingSounds = pendingSounds.filter { onlyLooping && !$0.value }
            sounds.stop(onlyLooping: onlyLooping)
        case let .poseEntered(pose):
            emit(.pose(pose: pose))
        case .interrupted:
            // The next showClip replaces the move as soon as it can.
            break
        case let .spent(name, used):
            emit(.reactionDone(name: name, used: used))
        case let .error(code, detail):
            if code == "clip_timeout" {
                // The clip asked for did not show in time and the engine
                // counts it unplayable (rules.md 9.3): drop it, and let a
                // later preload of its file mark it playable.
                for i in shows.indices where i != visible {
                    if let s = shows[i] { failed.insert(s.edge) }
                }
                dropPending()
            }
            emit(.error(code: code, detail: detail))
        }
    }

    private func emit(_ event: Event) {
        onEvent?(event)
    }

    // MARK: - Clips

    private func show(_ edge: String, move: Bool, rate: Double, poster: Bool = false) {
        token += 1
        var showing = Showing(edge: edge, move: move, rate: rate, token: token, poster: poster)
        current = token
        let file = delivery?.file(edge: edge, pixels: displayPixels)
        if let v = visible, let s = shows[v], s.edge == edge,
            s.poster || (!s.move && !move && s.key == file?.key)
        {
            // The same pose clip again (or the poster's clip): its next loop,
            // with no swap. A size that picks another file swaps to it instead.
            dropPending()
            if poster { return firstFrameShown() }
            showing.key = s.key
            shows[v] = showing
            let slot = stage.slots[v]
            slot.isMuted = !voiceOn(edge)
            slot.restart(rate: rate)
            if paused { slot.pause() }
            announce(showing)
            reportShown(showing)
            return
        }
        // Otherwise the other slot: load, wait for a decoded frame, swap.
        let slot = visible == 0 ? 1 : 0
        showing.key = file?.key
        shows[slot] = showing
        guard let file else {
            return clipFailed(showing, slot: slot)
        }
        loader.file(file.url, key: file.key) { [weak self] local in
            guard let self, self.current == showing.token else { return }
            guard let local else { return self.clipFailed(showing, slot: slot) }
            self.stage.slots[slot].load(local) { [weak self] ok in
                guard let self, self.current == showing.token else { return }
                if ok {
                    self.swap(to: slot, showing)
                } else {
                    self.clipFailed(showing, slot: slot)
                }
            }
        }
    }

    /// Makes the slot visible and hides the other in the same frame.
    private func swap(to slot: Int, _ showing: Showing) {
        stage.present(.slot(slot))
        if let old = visible, old != slot {
            stage.slots[old].pause()
            shows[old] = nil
        }
        visible = slot
        still = nil
        let s = stage.slots[slot]
        s.isMuted = !voiceOn(showing.edge)
        if showing.poster {
            firstFrameShown()
        } else {
            if !paused { s.play(rate: showing.rate) }
            announce(showing)
            reportShown(showing)
        }
    }

    /// Tells the engine the showing's first frame is on screen (rules.md
    /// 9.3): at the swap, or after the commands that restarted a pose clip in
    /// place. Once per showing; a showing a later show command overtook is
    /// not reported.
    private func reportShown(_ showing: Showing) {
        send { [weak self] engine, now in
            guard let self, self.current == showing.token else { return [] }
            return engine.clipShown(edge: showing.edge, now: now)
        }
    }

    private func announce(_ showing: Showing) {
        emit(.clip(clip: showing.edge, kind: showing.move ? .move : .pose, rate: showing.rate))
        if showing.move, let edge = delivery?.edges[showing.edge], let movement = edge.movement {
            emit(
                .movement(
                    edge: showing.edge, movement: movement, rate: showing.rate,
                    seconds: edge.duration / showing.rate))
        }
    }

    /// Forgets a clip loading in the hidden slot.
    private func dropPending() {
        for i in shows.indices where i != visible {
            shows[i] = nil
        }
    }

    private func clipFailed(_ showing: Showing, slot: Int) {
        if visible != slot, shows[slot]?.token == showing.token {
            shows[slot] = nil
        }
        if showing.poster { return firstFrameShown() }
        failed.insert(showing.edge)
        send { $0.clipFailed(edge: showing.edge, now: $1) }
    }

    private func slotEnded(_ slot: Int) {
        guard visible == slot, var s = shows[slot], s.token == current, !s.ended, !s.poster else { return }
        s.ended = true
        shows[slot] = s
        send { $0.clipEnded(edge: s.edge, now: $1) }
    }

    private func slotFailed(_ slot: Int) {
        guard visible == slot, let s = shows[slot], s.token == current, !s.ended, !s.poster else { return }
        clipFailed(s, slot: slot)
    }

    private func showStill(_ pose: String, poster: Bool = false) {
        token += 1
        let showing = Showing(edge: "still:\(pose)", move: false, rate: 1, token: token, poster: poster)
        current = token
        dropPending()
        guard let url = delivery?.stills[pose] else {
            return presentStill(nil, showing)
        }
        loader.image(url, key: "still:\(pose)") { [weak self] image in
            guard let self, !self.destroyed, self.current == showing.token else { return }
            self.presentStill(image, showing)
        }
    }

    private func presentStill(_ image: CGImage?, _ showing: Showing) {
        stage.present(.still(image))
        if let v = visible {
            stage.slots[v].pause()
            shows[v] = nil
        }
        visible = nil
        still = showing
        if showing.poster {
            firstFrameShown()
        } else {
            emit(.clip(clip: showing.edge, kind: .still, rate: showing.rate))
        }
    }

    /// Decodes the clip that most likely shows next in the hidden slot, so it
    /// swaps in on time: the decided next clip; during a move, its target
    /// pose's loop; else the pose clip on screen, when the mascot's size now
    /// picks another file for its next loop.
    private func prepareNext() {
        guard started, !destroyed, let engine else { return }
        let hidden = visible == 0 ? 1 : 0
        guard shows[hidden] == nil else { return }
        let onScreen = visible.flatMap { shows[$0] }
        var next = engine.state().nextClip
        if next == nil, let s = onScreen {
            if !s.move {
                next = s.edge
            } else if let loop = loopAhead,
                delivery?.edges[loop]?.source == delivery?.edges[s.edge]?.target
            {
                next = loop
            }
        }
        guard let next, let file = delivery?.file(edge: next, pixels: displayPixels),
            onScreen?.edge != next || onScreen?.key != file.key
        else { return }
        loader.file(file.url, key: file.key) { [weak self] local in
            guard let self, !self.destroyed, let local, self.shows[hidden] == nil, self.visible != hidden else {
                return
            }
            self.stage.slots[hidden].load(local) { _ in }
        }
    }

    private func preload(_ edge: String) {
        if delivery?.edges[edge]?.isLoop == true {
            loopAhead = edge
        }
        guard let file = delivery?.file(edge: edge, pixels: displayPixels) else {
            return markUnplayable(edge)
        }
        loader.file(file.url, key: file.key) { [weak self] local in
            guard let self, !self.destroyed else { return }
            if local == nil {
                self.markUnplayable(edge)
            } else if self.failed.remove(edge) != nil {
                self.engine?.markPlayable(edge: edge)
            }
        }
    }

    private func markUnplayable(_ edge: String) {
        failed.insert(edge)
        engine?.markUnplayable(edge: edge)
    }

    // MARK: - Sound

    private func voiceOn(_ edge: String) -> Bool {
        sound && (delivery?.edges[edge]?.hasAudio ?? false)
    }

    private func muteVoices() {
        for (i, s) in shows.enumerated() {
            if let s {
                stage.slots[i].isMuted = !voiceOn(s.edge)
            }
        }
    }

    private func playSound(_ edge: String) {
        guard let sound = delivery?.edges[edge]?.sound else { return }
        soundToken += 1
        let t = soundToken
        pendingSounds[t] = sound.loop
        loader.file(sound.url, key: "sound:\(edge)") { [weak self] local in
            guard let self, !self.destroyed, self.pendingSounds.removeValue(forKey: t) != nil, let local else {
                return
            }
            self.sounds.play(local, volume: sound.volume, loop: sound.loop)
        }
    }

    // MARK: - The start

    /// Shows the first frame the start will show, without starting: a
    /// scratch engine replays the calls so far and starts. Called again when
    /// a call before the start changes it.
    private func showPoster() {
        guard !started, !destroyed else { return }
        firstFrameReady = false
        guard let scratch = try? MaskoEngine(delivery: json) else { return firstFrameShown() }
        for c in beforeStart {
            _ = c(scratch, 0)
        }
        for command in scratch.start(now: 0) {
            switch command {
            case let .showClip(edge, kind, rate):
                return show(edge, move: kind == .move, rate: rate, poster: true)
            case let .showStill(pose):
                return showStill(pose, poster: true)
            default:
                continue
            }
        }
        firstFrameShown()
    }

    /// The first frame is on screen (or cannot be shown): the mascot starts
    /// with `autoPlay`, else on `play()`.
    private func firstFrameShown() {
        firstFrameReady = true
        if autoPlay && !started {
            start()
        }
    }

    /// Play time 0 (rules.md 11.1). The start shows the frame already on
    /// screen and plays it on.
    private func start() {
        started = true
        appPaused = false
        clock.run(at: host.uptime())
        send { engine, _ in engine.start(now: 0) }
        let later = afterStart
        afterStart = []
        later.forEach(send)
        scheduleRefresh()
        // Off screen already: the start waits there.
        applyPause()
    }

    // MARK: - Signed links

    /// Asks for fresh links `refreshLead` before the signed ones expire.
    private func scheduleRefresh() {
        refreshTimer?.cancel()
        refreshTimer = nil
        guard getDelivery != nil, !destroyed, let expires = delivery?.expiresAt else { return }
        let left = expires.timeIntervalSince(host.date())
        refresh(in: left > 2 * Self.refreshLead ? left - Self.refreshLead : max(left / 2, 0))
    }

    private func refresh(in delay: TimeInterval) {
        refreshTimer?.cancel()
        refreshTimer = host.at(host.date().addingTimeInterval(delay)) { [weak self] in
            self?.refresh()
        }
    }

    /// Asks the app for a fresh delivery and takes its links
    /// (`updateDelivery`). The engine keeps running, and files already
    /// fetched keep playing.
    @discardableResult
    func refresh() -> Task<Void, Never> {
        refreshTimer?.cancel()
        refreshTimer = nil
        let getDelivery = self.getDelivery
        return Task { @MainActor [weak self] in
            guard let getDelivery else { return }
            let json: Data
            do {
                json = try await getDelivery()
            } catch {
                guard let self, !self.destroyed else { return }
                self.emit(.error(code: "refresh_failed", detail: String(describing: error)))
                self.refresh(in: Self.refreshRetry)
                return
            }
            guard let self, !self.destroyed else { return }
            let before = self.delivery?.expiresAt
            guard self.updateDelivery(json) else {
                return self.refresh(in: Self.refreshRetry)
            }
            // Links that do not last longer are asked for again, not in a
            // tight loop.
            if let before, let after = self.delivery?.expiresAt, after <= before {
                self.refresh(in: Self.refreshRetry)
            }
        }
    }

    // MARK: - A canvas from its URL

    /// Remembers a value the app set, to carry it over to a newer release.
    private func hold(_ name: String, _ value: Value) {
        guard !destroyed else { return }
        values[name] = value
    }

    private static func setCall(_ name: String, _ value: Value) -> Call {
        switch value {
        case let .bool(b): return { $0.setBool(name: name, value: b, instant: false, now: $1) }
        case let .number(n): return { $0.setNumber(name: name, value: n, instant: false, now: $1) }
        }
    }

    /// Asks the canvas URL for its latest release (player.md, "A canvas from
    /// its URL"): the first delivery to play, fresh links for the release
    /// playing, or a newer release, kept whole before it takes over. Then
    /// again in an hour; a first delivery that does not come is asked for
    /// again 30 seconds later, then less and less often.
    @discardableResult
    func check() -> Task<Void, Never> {
        checkTimer?.cancel()
        checkTimer = nil
        let task = Task { @MainActor [weak self] in
            guard let release = self?.release else { return }
            do {
                let latest = try await release.latest()
                guard let self, !self.destroyed else { return }
                let fresh = try Delivery(json: latest)
                if self.engine == nil {
                    // Nothing plays yet: this one plays now, from its links,
                    // while every file is kept.
                    self.first(latest)
                    if self.engine != nil { try await release.keep(latest) }
                } else if self.delivery?.sameRelease(as: fresh) == true {
                    self.updateDelivery(latest)
                    // Completes the kept files when some are missing.
                    try await release.keep(latest)
                } else if let refused = Self.refusal(of: latest) {
                    // The release playing goes on.
                    self.emit(.error(code: refused.code, detail: refused.detail))
                } else {
                    try await release.keep(latest)
                    guard !self.destroyed else { return }
                    self.waiting = latest
                    if !self.started { self.switchRelease() }
                }
                self.retry = Self.refreshRetry
                self.checkAgain(in: Self.checkEvery)
            } catch {
                guard let self, !self.destroyed else { return }
                self.emit(.error(code: "refresh_failed", detail: String(describing: error)))
                if self.engine == nil {
                    self.checkAgain(in: self.retry)
                    self.retry = min(self.retry * 2, Self.checkEvery)
                } else {
                    self.checkAgain(in: Self.checkEvery)
                }
            }
        }
        checking = task
        return task
    }

    /// Why the engine refuses a delivery, if it does.
    private static func refusal(of data: Data) -> (code: String, detail: String?)? {
        do {
            _ = try MaskoEngine(delivery: String(decoding: data, as: UTF8.self))
            return nil
        } catch let LoadFailure.Refused(code, detail) {
            return (code, detail)
        } catch {
            return ("unreadable", String(describing: error))
        }
    }

    private func checkAgain(in delay: TimeInterval) {
        checkTimer?.cancel()
        checkTimer = host.at(host.date().addingTimeInterval(delay)) { [weak self] in
            self?.check()
        }
    }

    /// The first delivery of a canvas read from its URL. The values the app
    /// set meanwhile choose the first pose (rules.md 11.1).
    private func first(_ data: Data) {
        load(data)
        engine?.seed(seed: UInt64.random(in: 1...UInt64.max))
        if let refusal {
            return emit(.error(code: refusal.code, detail: refusal.detail))
        }
        for c in values.map({ Self.setCall($0.key, $0.value) }) {
            beforeStart.append(c)
            send(c)
        }
        scheduleRefresh()
        showPoster()
    }

    /// Whether this pose clip is the one on screen starting again on the
    /// initial pose: the moment a newer release can take over unseen, since
    /// its own start shows that pose too.
    private func restartsInitialLoop(_ edge: String) -> Bool {
        guard let v = visible, let s = shows[v], s.edge == edge, !s.move, !s.poster,
            let e = delivery?.edges[edge], let pose = delivery?.initialNode
        else { return false }
        return e.source == pose && e.target == pose
    }

    /// Plays the waiting release from now on. Before the start it gives the
    /// first frame; after it, it starts where the initial pose's loop would
    /// have started again, the picture on screen staying until its first clip
    /// swaps in, and the values the app set carry over. A release the engine
    /// refuses is reported and the one playing goes on (false).
    @discardableResult
    private func switchRelease() -> Bool {
        guard let data = waiting, !destroyed else { return false }
        waiting = nil
        let before = (json: json, engine: engine, delivery: delivery)
        load(data)
        guard let engine, refusal == nil else {
            let refused = refusal
            json = before.json
            self.engine = before.engine
            delivery = before.delivery
            refusal = nil
            if let refused { emit(.error(code: refused.code, detail: refused.detail)) }
            return false
        }
        engine.seed(seed: UInt64.random(in: 1...UInt64.max))
        failed = []
        loopAhead = nil
        pendingSounds = [:]
        if !started {
            for c in beforeStart {
                _ = c(engine, 0)
            }
            if booted { showPoster() }
            return true
        }
        sounds.stop(onlyLooping: true)
        dropPending()
        if let v = visible { shows[v] = nil }
        still = nil
        clock = PlayClock()
        clock.run(at: host.uptime())
        let carried = values.map { Self.setCall($0.key, $0.value) }
        let start: Call = { engine, now in engine.start(now: now) }
        queue.insert(contentsOf: [start] + carried, at: 0)
        return true
    }
}
