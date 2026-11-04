import AVFoundation
import Metal
import XCTest

@testable import MaskoPlayer

/// The real stage, watched: every swap records whether the slot held a
/// decoded frame at that moment.
@MainActor
final class SpyStage: Stage {
    let real = AVStage()
    var slots: [VideoSlot] { real.slots }
    private(set) var swaps: [(slot: Int, frame: CVPixelBuffer?, layerReady: Bool)] = []
    /// For AV1 clips: whether the slot had drawn the clip's first frame.
    private(set) var av1Swaps: [(slot: Int, drawn: Bool)] = []

    func present(_ content: StageContent) {
        if case let .slot(i) = content {
            let slot = real.clipSlots[i].hevc
            swaps.append((i, slot.firstFrame, slot.layer.isReadyForDisplay))
            av1Swaps.append((i, real.clipSlots[i].av1.firstFrameShown))
        }
        real.present(content)
    }
}

/// Real HEVC-with-alpha clips on AVFoundation (player.md, test 3).
@MainActor
final class RealVideoTests: XCTestCase {
    let clips = Bundle.module.url(forResource: "Clips", withExtension: nil)!

    /// The real-video delivery; `av1` names the stacked AV1 clips in `videos.av1`.
    func delivery(av1: Bool = false) -> Data {
        let key = av1 ? "av1" : "hevc"
        let loop = clips.appendingPathComponent(av1 ? "loop-av1.mp4" : "loop.mov").absoluteString
        let move = clips.appendingPathComponent(av1 ? "move-av1.mp4" : "move.mov").absoluteString
        return Data(
            """
            {
              "deliverySchemaVersion": 1,
              "playerContract": 1,
              "requires": [],
              "target": "apple",
              "mode": "selected",
              "runtimeGraph": {
                "runtimeGraphSchemaVersion": 1,
                "version": "2.1",
                "name": "Real video",
                "initialNode": "idle",
                "autoPlay": true,
                "nodes": [{"id": "idle", "name": "Idle"}, {"id": "wave", "name": "Wave"}],
                "edges": [
                  {"id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true, "duration": 0.5,
                   "videos": {"\(key)": "\(loop)"}},
                  {"id": "wave-loop", "source": "wave", "target": "wave", "isLoop": true, "duration": 0.5,
                   "videos": {"\(key)": "\(loop)"}},
                  {"id": "to-wave", "source": "idle", "target": "wave", "isLoop": false, "duration": 0.4,
                   "conditions": [{"input": "behavior::pointerHover"}], "videos": {"\(key)": "\(move)"},
                   "movement": {"dx": 10}},
                  {"id": "wave-back", "source": "wave", "target": "idle", "isLoop": false, "duration": 0.4,
                   "conditions": [{"input": "behavior::pointerHover", "value": false}], "videos": {"\(key)": "\(move)"}}
                ]
              }
            }
            """.utf8)
    }

    /// Alpha of one pixel of a BGRA frame.
    func alpha(_ frame: CVPixelBuffer, x: Int, y: Int) -> UInt8 {
        CVPixelBufferLockBaseAddress(frame, .readOnly)
        defer { CVPixelBufferUnlockBaseAddress(frame, .readOnly) }
        let base = CVPixelBufferGetBaseAddress(frame)!.assumingMemoryBound(to: UInt8.self)
        return base[y * CVPixelBufferGetBytesPerRow(frame) + x * 4 + 3]
    }

    func testTheFileCacheReadsALoneClipsLengthAndSoundTrack() async throws {
        let loop = clips.appendingPathComponent("loop.mov")
        let asset = AVURLAsset(url: loop)
        let expected = try await asset.load(.duration).seconds
        let voiced = try await !asset.loadTracks(withMediaType: .audio).isEmpty
        let cache = FileCache()
        let file: ClipFile? = await withCheckedContinuation { done in
            cache.clipFile(loop, key: "clip") { done.resume(returning: $0) }
        }
        XCTAssertEqual(try XCTUnwrap(file).seconds, expected, accuracy: 0.001)
        XCTAssertEqual(file?.voiced, voiced)
        XCTAssertGreaterThan(expected, 0)
        let missing: ClipFile? = await withCheckedContinuation { done in
            cache.clipFile(clips.appendingPathComponent("none.mov"), key: "missing") { done.resume(returning: $0) }
        }
        XCTAssertNil(missing)
    }

    func testTheClipFilesHaveAnAlphaChannel() async throws {
        let asset = AVURLAsset(url: clips.appendingPathComponent("loop.mov"))
        let tracks = try await asset.loadTracks(withMediaType: .video)
        let track = try XCTUnwrap(tracks.first)
        let formats = try await track.load(.formatDescriptions)
        let format = try XCTUnwrap(formats.first)
        XCTAssertEqual(CMFormatDescriptionGetMediaSubType(format), kCMVideoCodecType_HEVC)
        let extensions = CMFormatDescriptionGetExtensions(format) as? [String: Any]
        XCTAssertEqual(extensions?[kCMFormatDescriptionExtension_ContainsAlphaChannel as String] as? Bool, true)
    }

    func testRealClipsSwapOnADecodedFrameAndEndByTheirNotification() throws {
        let stage = SpyStage()
        let player = MaskoPlayer(
            delivery: delivery(), autoPlay: nil, stage: stage, loader: FileCache(), sounds: AVSounds(),
            host: .real)
        defer { player.destroy() }
        var log: [(event: String, at: Date)] = []
        var waiting: (String, Int, XCTestExpectation)?
        player.onEvent = { event in
            let text: String
            switch event {
            case let .pose(p): text = "pose:\(p)"
            case let .clip(c, _, _): text = "clip:\(c)"
            case let .reactionDone(n, _): text = "done:\(n)"
            case let .movement(e, _, _, _): text = "movement:\(e)"
            case let .error(code, _): text = "error:\(code)"
            }
            log.append((text, Date()))
            if let (want, count, expectation) = waiting, log.filter({ $0.event == want }).count >= count {
                waiting = nil
                expectation.fulfill()
            }
        }
        func wait(for event: String, count: Int = 1) {
            let e = expectation(description: "\(event) x\(count)")
            waiting = (event, count, e)
            self.wait(for: [e], timeout: 5)
        }

        // The first clip shows once a frame is decoded, then loops on the end
        // notification: no swap, and well before the safety net (1.5 s).
        player.boot()
        wait(for: "clip:idle-loop", count: 2)
        let shows = log.filter { $0.event == "clip:idle-loop" }.map(\.at)
        let loop = shows[1].timeIntervalSince(shows[0])
        XCTAssertGreaterThan(loop, 0.4)
        XCTAssertLessThan(loop, 1.2, "the end notification, not the safety net, ended the clip")
        XCTAssertEqual(stage.swaps.map(\.slot), [0], "a loop restarts in its slot")
        let first = try XCTUnwrap(stage.swaps[0].frame, "the swap waited for a decoded frame")
        XCTAssertEqual(alpha(first, x: 2, y: 2), 0, "transparent where the clip is transparent")
        XCTAssertEqual(alpha(first, x: 48, y: 48), 255, "opaque where the clip is opaque")

        // An instant move swaps to the other slot after its first frame.
        player.set("behavior::pointerHover", true, instant: true)
        wait(for: "clip:wave-loop")
        XCTAssertEqual(stage.swaps.map(\.slot), [0, 1, 0])
        XCTAssertEqual(
            log.map(\.event).filter { !$0.hasPrefix("clip:idle-loop") },
            ["pose:idle", "clip:to-wave", "movement:to-wave", "pose:wave", "clip:wave-loop"])

        // The pointer leaving with instant starts the way back at once
        // (rules.md 5.8), and it swaps in on a decoded frame too.
        player.set("behavior::pointerHover", false, instant: true)
        XCTAssertEqual(player.engine?.state().clip, "wave-back")
        wait(for: "clip:idle-loop", count: 3)
        XCTAssertEqual(stage.swaps.map(\.slot), [0, 1, 0, 1, 0])
        XCTAssertTrue(stage.swaps.allSatisfy { $0.frame != nil }, "every swap had a decoded frame")
        #if os(macOS)
        // On iOS a layer outside any window may never say so; the slot then
        // swaps a moment after the decoded frame.
        XCTAssertTrue(stage.swaps.allSatisfy(\.layerReady), "and its layer had it ready for display")
        #endif
        XCTAssertFalse(log.contains { $0.event.hasPrefix("error:") }, "\(log.map(\.event))")
        let position = player.position()
        XCTAssertEqual(position.clip, "idle-loop")
        XCTAssertLessThanOrEqual(position.time, 0.5)
    }

    func testAV1ClipsDecodeToTransparentPictures() async throws {
        let loop = clips.appendingPathComponent("loop-av1.mp4")
        XCTAssertTrue(AV1Clip.isAV1(loop))
        XCTAssertFalse(AV1Clip.isAV1(clips.appendingPathComponent("loop.mov")))
        let clip = try await AV1Clip.read(loop)
        XCTAssertEqual(clip.frames.count, 15)
        XCTAssertEqual(clip.duration, 0.5, accuracy: 0.01)
        XCTAssertEqual(clip.width, 96)
        XCTAssertEqual(clip.height, 192)
        XCTAssertNotNil(clip.sound, "the voiced clip's sound is read from the same file")
        XCTAssertEqual(clip.frameIndex(at: 0), 0)
        XCTAssertEqual(clip.frameIndex(at: 0.25), 7)
        XCTAssertEqual(clip.frameIndex(at: 9), 14)

        // Decoded (dav1d here, or the device's AV1 decoder) and drawn by the
        // shader: transparent in the corner, opaque orange in the centre.
        let renderer = try XCTUnwrap(StackedRenderer.shared)
        let decoder = try XCTUnwrap(AV1Decoder(clip: clip, device: renderer.device))
        decoder.want(upTo: 0)
        var frame: DecodedFrame?
        for _ in 0..<500 where frame == nil {
            frame = decoder.take(upTo: 0, after: -1)?.1
            try await Task.sleep(nanoseconds: 2_000_000)
        }
        let picture = try XCTUnwrap(frame, "frame 0 decoded")
        let description = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .bgra8Unorm, width: 96, height: 96, mipmapped: false)
        description.usage = [.renderTarget, .shaderRead]
        description.storageMode = .shared
        let target = try XCTUnwrap(renderer.device.makeTexture(descriptor: description))
        let drawn: Bool = await withCheckedContinuation { done in
            renderer.render(picture, into: target) { done.resume(returning: $0) }
        }
        XCTAssertTrue(drawn)
        var pixels = [UInt8](repeating: 0, count: 96 * 96 * 4)
        target.getBytes(&pixels, bytesPerRow: 96 * 4, from: MTLRegionMake2D(0, 0, 96, 96), mipmapLevel: 0)
        func bgra(_ x: Int, _ y: Int) -> [UInt8] { Array(pixels[(y * 96 + x) * 4..<(y * 96 + x) * 4 + 4]) }
        XCTAssertEqual(bgra(2, 2)[3], 0, "transparent where the clip is transparent")
        let centre = bgra(48, 48)
        XCTAssertEqual(centre[3], 255, "opaque where the clip is opaque")
        // #F95D02, give or take the encoding.
        XCTAssertEqual(Int(centre[2]), 0xF9, accuracy: 12)
        XCTAssertEqual(Int(centre[1]), 0x5D, accuracy: 12)
        XCTAssertEqual(Int(centre[0]), 0x02, accuracy: 12)
    }

    func testAVoicedAV1ClipsPictureWaitsForItsSoundToBeHeard() async throws {
        let url = clips.appendingPathComponent("loop-av1.mp4")
        let clip = try await AV1Clip.read(url)
        XCTAssertEqual(clip.soundStart, 0, accuracy: 0.001, "picture and sound start together in this file")
        let sound = try XCTUnwrap(clip.sound)
        let audio = SlotAudio(file: sound)
        guard audio.prepare() else { throw XCTSkip("no sound device") }
        // The output's delay is real and small on built-in speakers, long on Bluetooth.
        let delay = audio.delay
        XCTAssertGreaterThanOrEqual(delay, 0.05)
        XCTAssertLessThan(delay, 1)
        let asked = CACurrentMediaTime()
        let heard = try XCTUnwrap(audio.play(from: 0, rate: 1))
        XCTAssertEqual(heard - asked, delay, accuracy: 0.01, "heard after the lead and the speakers' delay")
        audio.stop()

        // A slot playing the clip with its sound holds the first frame until the sound is heard.
        let slot = AV1Slot()
        let loaded: Bool = await withCheckedContinuation { done in slot.load(url) { done.resume(returning: $0) } }
        XCTAssertTrue(loaded)
        slot.isMuted = false
        slot.play(rate: 1)
        XCTAssertEqual(slot.clipTime, 0, accuracy: 0.0001, "the picture waits for the sound")
        try await Task.sleep(nanoseconds: UInt64((delay + 0.15) * 1_000_000_000))
        XCTAssertGreaterThan(slot.clipTime, 0.1, "then plays with it")
        XCTAssertLessThan(slot.clipTime, 0.25)
        slot.unload()

        // Muted, nothing to wait for: the picture starts at once.
        let silent = AV1Slot()
        let ready: Bool = await withCheckedContinuation { done in silent.load(url) { done.resume(returning: $0) } }
        XCTAssertTrue(ready)
        silent.play(rate: 1)
        try await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertGreaterThan(silent.clipTime, 0.05)
        silent.unload()
    }

    func testAnAV1LoopRestartsWhileFramesAreStillAskedFor() async throws {
        let clip = try await AV1Clip.read(clips.appendingPathComponent("loop-av1.mp4"))
        let renderer = try XCTUnwrap(StackedRenderer.shared)
        let decoder = try XCTUnwrap(AV1Decoder(clip: clip, device: renderer.device))
        let last = clip.frames.count - 1
        for loop in 0..<30 {
            // A playing clip asks for frames on every tick, so some requests
            // are still waiting when it loops; the new loop starts a few
            // frames ahead, then asks for the rest as it plays.
            for index in 0...last { decoder.want(upTo: index) }
            decoder.reset()
            decoder.want(upTo: 3)
            var shown = -1
            for _ in 0..<1000 where shown < last && !decoder.failed {
                if let (index, _) = decoder.take(upTo: last, after: shown) { shown = index }
                decoder.want(upTo: min(shown + 4, last))
                try await Task.sleep(nanoseconds: 1_000_000)
            }
            XCTAssertFalse(decoder.failed, "loop \(loop) decoded every frame")
            XCTAssertEqual(shown, last, "loop \(loop) reached its last frame")
            if decoder.failed { break }
        }
    }

    func testRealAV1ClipsSwapOnADrawnFrameAndEndOnTheirLength() throws {
        let stage = SpyStage()
        let player = MaskoPlayer(
            delivery: delivery(av1: true), autoPlay: nil, stage: stage, loader: FileCache(), sounds: AVSounds(),
            host: .real)
        defer { player.destroy() }
        var log: [(event: String, at: Date)] = []
        var waiting: (String, Int, XCTestExpectation)?
        player.onEvent = { event in
            let text: String
            switch event {
            case let .pose(p): text = "pose:\(p)"
            case let .clip(c, _, _): text = "clip:\(c)"
            case let .reactionDone(n, _): text = "done:\(n)"
            case let .movement(e, _, _, _): text = "movement:\(e)"
            case let .error(code, _): text = "error:\(code)"
            }
            log.append((text, Date()))
            if let (want, count, expectation) = waiting, log.filter({ $0.event == want }).count >= count {
                waiting = nil
                expectation.fulfill()
            }
        }
        func wait(for event: String, count: Int = 1) {
            let e = expectation(description: "\(event) x\(count)")
            waiting = (event, count, e)
            self.wait(for: [e], timeout: 5)
        }

        // The first clip shows once its first frame is drawn, then loops on its
        // length, well before the safety net (1.5 s).
        player.boot()
        wait(for: "clip:idle-loop", count: 2)
        let shows = log.filter { $0.event == "clip:idle-loop" }.map(\.at)
        let loop = shows[1].timeIntervalSince(shows[0])
        XCTAssertGreaterThan(loop, 0.4)
        XCTAssertLessThan(loop, 1.2, "the clip's end, not the safety net, ended it")
        XCTAssertEqual(stage.av1Swaps.map(\.slot), [0], "a loop restarts in its slot")

        // An instant move swaps to the other slot after its first frame, and back.
        player.set("behavior::pointerHover", true, instant: true)
        wait(for: "clip:wave-loop")
        XCTAssertEqual(
            log.map(\.event).filter { !$0.hasPrefix("clip:idle-loop") },
            ["pose:idle", "clip:to-wave", "movement:to-wave", "pose:wave", "clip:wave-loop"])
        player.set("behavior::pointerHover", false, instant: true)
        wait(for: "clip:idle-loop", count: 3)
        XCTAssertEqual(stage.av1Swaps.map(\.slot), [0, 1, 0, 1, 0])
        XCTAssertTrue(stage.av1Swaps.allSatisfy(\.drawn), "every swap had its first frame drawn")
        XCTAssertFalse(log.contains { $0.event.hasPrefix("error:") }, "\(log.map(\.event))")
        let position = player.position()
        XCTAssertEqual(position.clip, "idle-loop")
        XCTAssertLessThanOrEqual(position.time, 0.5)
    }
}
