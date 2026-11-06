import Foundation
import XCTest

@testable import MaskoPlayer

/// The player against a fake video layer (player.md, test 2): two slots, a
/// swap only after the first frame, loop restarts, end and failure reports,
/// the preload cache, sounds, events, position, pause and play, and signed
/// link refresh.
@MainActor
final class PlayerTests: XCTestCase {
    var stage: FakeStage!
    var loader: FakeLoader!
    var sounds: FakeSounds!
    var host: FakeHost!
    var events: [String] = []
    var heard: [MaskoPlayer.Event] = []

    var a: FakeSlot { stage.fakeSlots[0] }
    var b: FakeSlot { stage.fakeSlots[1] }

    nonisolated static let deliveryText = """
        {
          "deliverySchemaVersion": 1,
          "playerContract": 1,
          "requires": ["any-state"],
          "target": "apple",
          "mode": "selected",
          "runtimeGraph": {
            "runtimeGraphSchemaVersion": 1,
            "version": "2.1",
            "name": "Test",
            "initialNode": "idle",
            "autoPlay": true,
            "nodes": [
              {"id": "idle", "name": "Idle", "transparentThumbnailUrl": "https://cdn.test/idle.png"},
              {"id": "working", "name": "Working", "transparentThumbnailUrl": "https://cdn.test/working.png"},
              {"id": "napping", "name": "Napping", "transparentThumbnailUrl": "https://cdn.test/nap.png"}
            ],
            "inputs": [{"name": "behavior::working", "type": "boolean", "default": false}],
            "edges": [
              {"id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true, "duration": 2,
               "videos": {"hevc": "https://cdn.test/idle-loop.mov", "webm": "https://cdn.test/idle-loop.webm"},
               "sound": {"url": "https://cdn.test/hum.mp3", "loop": true, "volume": 0.5}},
              {"id": "work-loop", "source": "working", "target": "working", "isLoop": true, "duration": 2,
               "videos": {"hevc": "https://cdn.test/work-loop.mov"}, "hasAudio": true},
              {"id": "to-work", "source": "idle", "target": "working", "isLoop": false, "duration": 1,
               "conditions": [{"input": "behavior::working"}],
               "videos": {"hevc": "https://cdn.test/to-work.mov"},
               "sound": {"url": "https://cdn.test/whoosh.mp3"},
               "movement": {"dx": 120, "easing": "linear"}},
              {"id": "to-idle", "source": "working", "target": "idle", "isLoop": false, "duration": 1,
               "conditions": [{"input": "behavior::working", "value": false}],
               "videos": {"hevc": "https://cdn.test/to-idle.mov"}},
              {"id": "wave", "source": "idle", "target": "working", "isLoop": false, "duration": 1, "priority": 1,
               "conditions": [{"input": "behavior::pointerHover"}],
               "videos": {"hevc": "https://cdn.test/wave.mov"}},
              {"id": "to-nap", "source": "idle", "target": "napping", "isLoop": false, "duration": 1,
               "conditions": [{"input": "action::nap"}],
               "videos": {"hevc": "https://cdn.test/to-nap.mov"}},
              {"id": "nap-back", "source": "napping", "target": "idle", "isLoop": false, "duration": 1,
               "conditions": [{"input": "node::loopCount", "op": ">=", "value": 1}],
               "videos": {"hevc": "https://cdn.test/nap-back.mov"}},
              {"id": "go-work", "source": "*", "target": "working", "isLoop": false, "duration": 0,
               "conditions": [{"input": "behavior::focus"}], "videos": {}}
            ]
          }
        }
        """

    nonisolated static func delivery() -> JSON {
        try! JSON(data: Data(deliveryText.utf8))
    }

    func makePlayer(_ delivery: JSON = PlayerTests.delivery(), autoPlay: Bool? = nil) -> MaskoPlayer {
        makePlayer(Data(delivery.text.utf8), autoPlay: autoPlay)
    }

    func makePlayer(_ data: Data, autoPlay: Bool? = nil) -> MaskoPlayer {
        stage = FakeStage()
        loader = FakeLoader()
        sounds = FakeSounds()
        host = FakeHost()
        events = []
        heard = []
        let player = MaskoPlayer(
            delivery: data, autoPlay: autoPlay, stage: stage, loader: loader, sounds: sounds, host: host.host)
        listen(player)
        return player
    }

    /// A player for one clip, `wave.mov`, whose length the fake loader reads from `lengths`.
    func makeClipPlayer(
        lengths: [String: TimeInterval] = ["wave.mov": 3], voiced: Bool = false, autoPlay: Bool? = nil
    ) -> MaskoPlayer {
        stage = FakeStage()
        loader = FakeLoader()
        loader.lengths = lengths
        loader.voiced = voiced ? ["wave.mov"] : []
        sounds = FakeSounds()
        host = FakeHost()
        events = []
        heard = []
        let player = MaskoPlayer(
            clip: URL(string: "https://cdn.test/wave.mov")!, autoPlay: autoPlay, stage: stage, loader: loader,
            sounds: sounds, host: host.host)
        listen(player)
        return player
    }

    func listen(_ player: MaskoPlayer) {
        player.onEvent = { [unowned self] event in
            self.heard.append(event)
            switch event {
            case let .pose(p): self.events.append("pose:\(p)")
            case let .clip(c, _, _): self.events.append("clip:\(c)")
            case let .reactionDone(name, used): self.events.append("done:\(name)\(used ? "" : ":unused")")
            case let .movement(edge, _, _, _): self.events.append("movement:\(edge)")
            case let .error(code, _): self.events.append("error:\(code)")
            }
        }
    }

    /// Starts the player and lets the first clip decode.
    func started(_ delivery: JSON = PlayerTests.delivery()) -> MaskoPlayer {
        let player = makePlayer(delivery)
        player.boot()
        a.decodeFirstFrame()
        events = []
        return player
    }

    /// Ends the clip on screen after `seconds` of play.
    func end(_ slot: FakeSlot, after seconds: TimeInterval) {
        host.advance(seconds)
        slot.end()
    }

    func now(_ player: MaskoPlayer) -> UInt64 {
        player.engine!.state().clock
    }

    // MARK: - Two slots

    func testTheMascotStartsWhenItsFirstFrameIsDecoded() {
        let player = makePlayer()
        player.boot()
        XCTAssertEqual(a.log, ["load idle-loop.mov"], "the first clip loads")
        XCTAssertTrue(stage.presented.isEmpty, "nothing shows before a frame is decoded")
        XCTAssertNil(player.engine!.state().pose, "and the mascot has not started")
        host.advance(5)
        a.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0)])
        XCTAssertEqual(a.log, ["load idle-loop.mov", "restart 1.0"])
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"])
        XCTAssertEqual(player.engine!.state().clock, 0, "play time 0 is when the first frame shows")
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
        host.advance(2)
        a.end()
        XCTAssertEqual(now(player), 2000)
    }

    func testALoneClipLoopsAsOnePoseFromTheLengthOfItsFile() {
        let player = makeClipPlayer()
        player.boot()
        XCTAssertTrue(loader.requested("wave.mov"))
        XCTAssertEqual(a.log, ["load wave.mov"])
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:clip", "clip:clip"])
        end(a, after: 3)
        XCTAssertEqual(a.log, ["load wave.mov", "restart 1.0", "restart 1.0"], "the loop restarts in its slot")
        XCTAssertEqual(stage.presented, [.slot(0)])
        XCTAssertEqual(player.engine!.state().pose, "clip")
    }

    func testALoneClipsSafetyNetCountsTheLengthOfItsFile() {
        let player = makeClipPlayer()
        player.boot()
        a.decodeFirstFrame()
        host.advance(3.9)
        XCTAssertFalse(events.contains("error:missing_end"))
        host.advance(0.2)
        XCTAssertTrue(events.contains("error:missing_end"), "3 s + 1 s after the first frame")
    }

    func testALoneClipWhoseLengthCannotBeReadPlaysNothing() {
        let player = makeClipPlayer(lengths: [:])
        player.boot()
        XCTAssertEqual(events, ["error:clip_failed"])
        XCTAssertNil(player.engine)
        XCTAssertTrue(a.log.isEmpty)
    }

    func testPlayBeforeALoneClipIsReadStartsItOnceItShows() {
        let player = makeClipPlayer(autoPlay: false)
        player.play()
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:clip", "clip:clip"])
    }

    func testALoneClipsOwnVoiceIsHeardUntilSoundIsTurnedOff() {
        let player = makeClipPlayer(voiced: true)
        player.boot()
        a.decodeFirstFrame()
        XCTAssertFalse(a.isMuted, "sound is on by default: the file's voice is heard")
        player.sound = false
        XCTAssertTrue(a.isMuted)
    }

    func testALoneClipWithoutASoundTrackStaysMuted() {
        let player = makeClipPlayer()
        player.boot()
        a.decodeFirstFrame()
        XCTAssertTrue(player.sound)
        XCTAssertTrue(a.isMuted, "no sound track: the clip never asks for the audio")
    }

    func testPlayBeforeTheFirstFrameWaitsForIt() {
        let player = makePlayer(autoPlay: false)
        player.play()
        player.boot()
        XCTAssertNil(player.engine!.state().pose)
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"])
    }

    func testPauseBeforeTheFirstFrameKeepsTheMascotThere() {
        let player = makePlayer()
        player.pause()
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0)])
        XCTAssertNil(player.engine!.state().pose)
        player.play()
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"])
    }

    func testAFirstClipThatCannotLoadStillStartsTheMascot() {
        let player = makePlayer()
        loader.failing = ["idle-loop.mov"]
        player.boot()
        XCTAssertEqual(events, ["pose:idle", "error:clip_failed", "clip:still:idle"])
        XCTAssertEqual(player.engine!.state().clip, "still:idle")
    }

    func testThePoseClipShownAgainRestartsWithoutASwap() {
        let player = started()
        end(a, after: 2)
        XCTAssertEqual(now(player), 2000)
        XCTAssertEqual(a.log, ["load idle-loop.mov", "restart 1.0", "restart 1.0"])
        XCTAssertEqual(stage.presented, [.slot(0)], "no swap for the next loop")
        XCTAssertTrue(b.log.isEmpty)
        XCTAssertEqual(events, ["clip:idle-loop"])
        XCTAssertEqual(player.engine!.value(name: "node::loopCount"), .number(value: 1))
    }

    func testAMoveLoadsInTheOtherSlotAndSwapsAfterItsFirstFrame() {
        let player = started()
        host.advance(0.5)
        player.set("working", true)
        // Decided for the end of the loop, and loaded ahead in the hidden slot.
        XCTAssertEqual(player.engine!.state().nextClip, "to-work")
        XCTAssertEqual(b.log, ["load to-work.mov"])
        end(a, after: 1.5)
        XCTAssertEqual(stage.presented, [.slot(0)], "the old clip stays until the new frame is decoded")
        XCTAssertFalse(b.playing)
        b.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)])
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
        XCTAssertEqual(
            a.log.suffix(2), ["pause", "load work-loop.mov"],
            "the old slot pauses after the swap, then takes the move's next loop")
        XCTAssertEqual(b.log, ["load to-work.mov", "play 1.0"])
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work"])
        XCTAssertEqual(player.position(), .init(clip: "to-work", time: 0))

        // The move's end enters its target; its loop comes in slot a.
        end(b, after: 1)
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work", "pose:working"])
        XCTAssertEqual(a.clip, "work-loop")
        a.decodeFirstFrame()
        XCTAssertEqual(stage.presented.last, .slot(0))
        XCTAssertEqual(events.last, "clip:work-loop")
    }

    func testAPreloadedClipIsLoadedOnlyOnce() {
        let player = started()
        player.set("working", true)
        b.decodeFirstFrame()  // the move, loaded ahead, is ready early
        XCTAssertEqual(stage.presented, [.slot(0)], "a prepared clip waits for its moment")
        end(a, after: 2)
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)], "and swaps in at once")
        XCTAssertEqual(b.log, ["load to-work.mov", "reuse to-work.mov", "play 1.0"])
    }

    func testThePreloadCacheIsAskedForEveryMoveOnEntry() {
        _ = started()
        XCTAssertTrue(loader.requested("wave.mov"))
        XCTAssertTrue(loader.requested("to-work.mov"))
        XCTAssertTrue(loader.requested("to-nap.mov"))
        XCTAssertEqual(loader.requests.first { $0.url.lastPathComponent == "to-work.mov" }?.key, "to-work")
    }

    func testAMoveThatStartsPreloadsItsTargetsMainLoop() {
        let player = started()
        XCTAssertFalse(loader.requested("work-loop.mov"))
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-work")
        XCTAssertTrue(loader.requested("work-loop.mov"))
    }

    func testTheTargetLoopDecodesDuringTheMoveAndSwapsInAtItsEnd() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        XCTAssertEqual(player.engine!.state().clip, "to-work")
        XCTAssertNil(player.engine!.state().nextClip, "nothing is decided during the move")
        XCTAssertEqual(a.clip, "work-loop", "the target's loop the engine preloads goes in the free slot")
        a.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)], "and waits there")
        end(b, after: 1)
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1), .slot(0)], "no wait at the move's end")
        XCTAssertEqual(a.log.suffix(2), ["reuse work-loop.mov", "play 1.0"])
        XCTAssertEqual(events.suffix(2), ["pose:working", "clip:work-loop"])
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
    }

    func testAClipEndIsReportedOncePerShowing() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        b.end()  // a second end signal for the same showing
        XCTAssertEqual(events.filter { $0 == "pose:working" }.count, 1)
        XCTAssertEqual(player.engine!.state().clip, "work-loop")
        // An end from the slot that is not on screen is not reported either.
        a.decodeFirstFrame()
        b.end()
        XCTAssertEqual(player.engine!.state().pose, "working")
        XCTAssertEqual(player.engine!.value(name: "node::loopCount"), .number(value: 0))
    }

    // MARK: - Failures

    func testAMoveThatFailsToLoadSkipsToItsTarget() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        b.failLoad()
        XCTAssertEqual(events, ["error:clip_failed", "pose:working"])
        XCTAssertEqual(player.engine!.state().clip, "work-loop")
        XCTAssertEqual(stage.presented, [.slot(0)], "the failed clip never shows")
    }

    func testAPreloadThatFailsMarksTheClipUnplayableUntilItLoads() {
        let player = makePlayer()
        loader.failing = ["to-work.mov"]
        player.boot()
        a.decodeFirstFrame()
        events = []
        player.set("working", true)
        end(a, after: 2)
        // The engine skips the move it knows cannot play.
        XCTAssertEqual(events, ["error:clip_failed", "pose:working"])
        XCTAssertFalse(b.log.contains("load to-work.mov"))
        // Back on idle the preload runs again, and works this time.
        loader.failing = []
        b.decodeFirstFrame()
        player.set("working", false)
        end(b, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-idle")
        a.decodeFirstFrame()
        end(a, after: 1)
        XCTAssertEqual(player.engine!.state().pose, "idle")
        b.decodeFirstFrame()
        events = []
        player.set("working", true)
        end(b, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-work", "playable again")
        XCTAssertEqual(a.file?.lastPathComponent, "to-work.mov")
        XCTAssertFalse(events.contains("error:clip_failed"))
    }

    func testAPoseClipThatFailsShowsTheStill() {
        let player = started()
        loader.failing = ["work-loop.mov"]
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work", "pose:working", "error:clip_failed", "clip:still:working"])
        XCTAssertEqual(stage.showing, .still(FakeLoader.image))
        XCTAssertEqual(b.log.last, "pause")
        XCTAssertEqual(player.position(), .init(clip: "still:working", time: 0))
    }

    func testAClipThatFailsWhilePlayingIsReported() {
        let player = started()
        a.failWhilePlaying()
        XCTAssertEqual(events, ["error:clip_failed", "clip:still:idle"])
        XCTAssertEqual(player.engine!.state().clip, "still:idle")
    }

    func testAPoseWithoutClipsShowsItsStillAndMovesOn() {
        let player = started()
        player.fire("nap")
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        XCTAssertEqual(events, ["clip:to-nap", "pose:napping", "clip:still:napping"])
        XCTAssertEqual(stage.showing, .still(FakeLoader.image))
        // The engine ends the still by itself after 1 s; the way back starts.
        host.advance(1.001)
        XCTAssertEqual(player.engine!.state().clip, "nap-back")
        XCTAssertEqual(a.clip, "nap-back")
        a.decodeFirstFrame()
        XCTAssertEqual(stage.showing, .slot(0))
    }

    // MARK: - Clock, wake and safety net

    func testTheWakeTimerRunsTheSafetyNet() {
        let player = started()
        XCTAssertEqual(host.pendingTimers, 1)
        host.advance(2.999)
        XCTAssertEqual(events, [])
        host.advance(0.002)
        XCTAssertEqual(events, ["error:missing_end", "clip:idle-loop"])
        XCTAssertEqual(now(player), 3000)
    }

    func testAClipIsReportedShownAtTheSwapAndAtEachLoop() {
        let player = started()
        XCTAssertEqual(player.engine!.state().shownAt, 0, "the first frame, played on in place at the start")
        XCTAssertEqual(player.engine!.nextWake(), 3000, "its safety net counts from there")
        end(a, after: 2)
        XCTAssertEqual(player.engine!.state().shownAt, 2000, "the next loop is on screen at once")
        XCTAssertEqual(player.engine!.nextWake(), 5000)
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-work")
        XCTAssertEqual(player.engine!.nextWake(), 14_000, "no safety net while the move loads")
        host.advance(0.5)
        b.decodeFirstFrame()
        XCTAssertEqual(player.engine!.state().shownAt, 4500, "on screen at the swap")
        XCTAssertEqual(player.engine!.nextWake(), 6500)
    }

    func testAShowingOvertakenBeforeItsReportIsNotReported() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        // An editor answers the move's clip event by playing the move again.
        player.onEvent = { [weak player] event in
            guard case .clip("to-work", _, _) = event, let player else { return }
            player.preview.showPose("idle")
            player.preview.playMove("to-work")
        }
        b.decodeFirstFrame()
        XCTAssertEqual(a.clip, "to-work", "the new showing loads in the other slot")
        XCTAssertEqual(player.engine!.nextWake(), 12_000, "and is not on screen yet")
    }

    func testASlowFirstFrameIsNotAMissingEnd() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        // The move's first frame takes longer than the move and its safety
        // net: the net counts from the swap.
        host.advance(2.5)
        XCTAssertEqual(events, [])
        b.decodeFirstFrame()
        host.advance(1.999)
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work"], "no missing end yet")
        host.advance(0.002)
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work", "error:missing_end", "pose:working"])
        XCTAssertEqual(now(player), 6500)
    }

    func testAClipThatNeverShowsTimesOutAfterTenSeconds() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(b.clip, "to-work")
        host.advance(9.999)
        XCTAssertEqual(events, [])
        XCTAssertEqual(stage.presented, [.slot(0)], "the frame before stays on screen meanwhile")
        host.advance(0.002)
        XCTAssertEqual(events, ["error:clip_timeout", "pose:working"])
        XCTAssertEqual(now(player), 12_000)
        XCTAssertEqual(player.engine!.state().clip, "work-loop")
        XCTAssertEqual(b.clip, "work-loop", "the move that timed out is dropped")
        b.decodeFirstFrame()
        XCTAssertEqual(events.last, "clip:work-loop")

        // Back on idle, the move's preload finds its file: it plays next time.
        player.set("working", false)
        end(b, after: 2)
        a.decodeFirstFrame()
        end(a, after: 1)
        XCTAssertEqual(player.engine!.state().pose, "idle")
        b.decodeFirstFrame()
        events = []
        player.set("working", true)
        end(b, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-work", "playable again")
        XCTAssertFalse(events.contains("error:clip_failed"))
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
    }

    func testPauseStopsTimeClipsSoundsAndHoldsCallsUntilPlay() {
        let player = started()
        host.advance(0.5)
        player.pause()
        XCTAssertEqual(a.log.last, "pause")
        XCTAssertEqual(sounds.log.last, "pause")
        XCTAssertEqual(host.pendingTimers, 0)
        player.set("working", true)
        player.fire("hello")
        XCTAssertEqual(player.engine!.value(name: "working"), .bool(value: false), "held until play")
        host.advance(10)
        XCTAssertEqual(player.engine!.state().clock, 0)
        player.play()
        XCTAssertEqual(a.log.last, "play 1.0")
        XCTAssertEqual(sounds.log.last, "resume")
        XCTAssertEqual(player.engine!.value(name: "working"), .bool(value: true))
        XCTAssertTrue(events.contains("done:action::hello:unused"), "the held fire reached the engine")
        XCTAssertEqual(now(player), 500, "play time resumes where it stopped")
        host.advance(1.5)
        a.end()
        XCTAssertEqual(now(player), 2000)
        XCTAssertEqual(player.engine!.state().clip, "to-work")
    }

    func testOffScreenPausesUntilBothTheViewAndTheAppLetItPlay() {
        let player = started()
        host.advance(0.5)
        player.setHostActive(false)
        XCTAssertEqual(a.log.last, "pause")
        XCTAssertEqual(sounds.log.last, "pause")
        XCTAssertEqual(host.pendingTimers, 0)
        player.fire("hello")
        host.advance(10)
        XCTAssertEqual(player.engine!.pendingFires(), [], "held while off screen")
        player.pause()
        player.setHostActive(true)
        XCTAssertEqual(player.now, 500, "the app's pause still holds")
        XCTAssertEqual(a.log.last, "pause")
        player.play()
        XCTAssertEqual(a.log.last, "play 1.0")
        XCTAssertTrue(events.contains("done:action::hello:unused"), "the held fire reached the engine")
        XCTAssertEqual(player.now, 500, "play time resumes where it stopped")
    }

    func testAStartOffScreenWaitsThere() {
        let player = makePlayer()
        player.setHostActive(false)
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(player.engine!.state().clip, "idle-loop", "the mascot started")
        XCTAssertEqual(host.pendingTimers, 0)
        host.advance(5)
        XCTAssertEqual(player.now, 0)
        player.setHostActive(true)
        host.advance(0.25)
        XCTAssertEqual(player.now, 250)
    }

    func testPosition() {
        let player = makePlayer()
        XCTAssertEqual(player.position(), .init(clip: nil, time: 0))
        player.boot()
        XCTAssertEqual(player.position(), .init(clip: nil, time: 0), "nothing on screen before the first frame")
        a.decodeFirstFrame()
        a.clipTime = 0.75
        XCTAssertEqual(player.position(), .init(clip: "idle-loop", time: 0.75))
    }

    // MARK: - Sounds

    func testEdgeSoundsLoopAndStopAsTheEngineSays() {
        let player = started()
        XCTAssertEqual(sounds.log, ["play hum 0.5 loop"])
        end(a, after: 2)
        XCTAssertEqual(sounds.log, ["play hum 0.5 loop"], "a looping sound is not started again")
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(sounds.log, ["play hum 0.5 loop", "stop all", "play whoosh 1.0"])
        b.decodeFirstFrame()
        end(b, after: 1)
        player.set("working", false)
        a.decodeFirstFrame()
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        XCTAssertEqual(player.engine!.state().clip, "idle-loop")
        XCTAssertEqual(sounds.log.last, "play hum 0.5 loop")
    }

    // MARK: - Instant (rules.md 5.8)

    func testAnInstantAnyStateStartEntersThePoseWithNoMoveClipMovementOrMoveSound() {
        // go-work, the Any State edge to working, on working and ranked above the move.
        let text = PlayerTests.deliveryText.replacingOccurrences(
            of: #""conditions": [{"input": "behavior::focus"}], "videos": {}"#,
            with: #""priority": 1, "conditions": [{"input": "behavior::working"}], "videos": {}"#)
        XCTAssertNotEqual(text, PlayerTests.deliveryText)
        let player = started(try! JSON(data: Data(text.utf8)))
        host.advance(0.5)
        player.set("working", true, instant: true)
        XCTAssertEqual(events, ["pose:working"], "the pose is entered mid-loop")
        XCTAssertEqual(b.clip, "work-loop", "the next clip is the pose clip, not the move")
        b.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:working", "clip:work-loop"])
        XCTAssertEqual(
            sounds.log, ["play hum 0.5 loop", "stop looping"], "no move sound; entering stops the looping one")
    }

    func testAnInstantMovePlaysItsOwnClipAtOnce() {
        let player = started()
        host.advance(0.5)
        player.set("working", true, instant: true)
        XCTAssertEqual(player.engine!.state().clip, "to-work", "mid-loop, not at its end")
        XCTAssertEqual(b.clip, "to-work")
        b.decodeFirstFrame()
        XCTAssertEqual(events, ["clip:to-work", "movement:to-work"])
        XCTAssertEqual(sounds.log, ["play hum 0.5 loop", "stop all", "play whoosh 1.0"])
    }

    func testAnInstantFirePlaysItsMoveAtOnce() {
        let player = started()
        player.set("score", 2, instant: true)
        player.set("pace", 1.5, instant: true)
        XCTAssertEqual(events, [])
        player.fire("nap", instant: true)
        XCTAssertEqual(player.engine!.state().clip, "to-nap")
        XCTAssertEqual(player.engine!.state().pose, "idle")
    }

    func testASoundStoppedBeforeItsFileArrivesNeverPlays() {
        let player = started()
        loader.holding = true
        player.reset()  // idle again: its looping sound is asked for
        player.set("behavior::pointerHover", true, instant: true)  // a move now stops every sound
        loader.release()
        XCTAssertEqual(sounds.log, ["play hum 0.5 loop", "stop looping", "stop all"])
        XCTAssertEqual(b.clip, "wave")
    }

    func testClipVoicesFollowTheSoundSetting() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        a.decodeFirstFrame()
        XCTAssertEqual(player.engine!.state().clip, "work-loop")
        XCTAssertFalse(a.isMuted, "sound is on by default, and work-loop has its own voice")
        player.sound = false
        XCTAssertTrue(a.isMuted)
        player.sound = true
        XCTAssertFalse(a.isMuted)
    }

    func testSoundOnKeepsClipsWithoutAVoiceMuted() {
        let player = started()
        player.sound = true
        XCTAssertTrue(a.isMuted, "idle-loop has no voice")
    }

    // MARK: - Events

    func testEvents() {
        let player = started()
        player.set("node::loopCount", 3)
        XCTAssertEqual(events, ["error:read_only"])
        player.fire("hello")
        end(a, after: 2)
        XCTAssertEqual(events, ["error:read_only", "done:action::hello:unused", "clip:idle-loop"])
        events = []
        player.set("behavior::pointerHover", true, instant: true)
        XCTAssertEqual(player.engine!.state().clip, "wave", "an instant move starts at once")
        b.decodeFirstFrame()
        XCTAssertEqual(events, ["clip:wave"], "no movement event for a move without movement")
    }

    func testEventsCarryTheSameDataAsOnTheWeb() {
        var delivery = PlayerTests.delivery()
        var edges = delivery["runtimeGraph"]!["edges"]!.array!
        edges[2]["speed"] = .int(2)
        delivery["runtimeGraph"]?["edges"] = .array(edges)
        let player = makePlayer(delivery)
        loader.failing = ["work-loop.mov"]
        player.boot()
        a.decodeFirstFrame()
        player.set("node::loopCount", 1)
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 0.5)
        XCTAssertEqual(
            heard,
            [
                .pose(pose: "idle"), .clip(clip: "idle-loop", kind: .pose, rate: 1),
                .error(code: "read_only", detail: "node::loopCount"),
                .clip(clip: "to-work", kind: .move, rate: 2),
                .movement(
                    edge: "to-work", movement: .object(["dx": .number(120), "easing": .string("linear")]),
                    rate: 2, seconds: 0.5),
                .pose(pose: "working"), .error(code: "clip_failed", detail: "work-loop"),
                .clip(clip: "still:working", kind: .still, rate: 1),
            ])
    }

    func testARefusedDeliveryReportsItsCodeAndPlaysNothing() {
        var delivery = PlayerTests.delivery()
        delivery["playerContract"] = .int(2)
        let player = makePlayer(delivery)
        player.boot()
        player.play()
        player.set("working", true)
        XCTAssertEqual(events, ["error:unsupported_contract"])
        XCTAssertEqual(heard, [.error(code: "unsupported_contract", detail: "playerContract 2")])
        XCTAssertTrue(a.log.isEmpty && b.log.isEmpty)
    }

    func testADeliveryThatIsNotJSONIsUnreadable() {
        let player = makePlayer(Data("{not json".utf8))
        player.boot()
        player.play()
        XCTAssertEqual(events, ["error:unreadable"])
        XCTAssertTrue(a.log.isEmpty && b.log.isEmpty)
        XCTAssertEqual(player.position(), .init(clip: nil, time: 0))
    }

    // MARK: - Start

    func testValuesSetBeforeTheStartChooseTheFirstPose() {
        let player = makePlayer()
        player.set("working", true)
        player.set("focus", true)
        player.boot()
        XCTAssertEqual(a.clip, "work-loop")
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:working", "clip:work-loop"])
    }

    func testWithAutoPlayOffTheFirstFrameShowsAndPlayStartsIt() {
        let player = makePlayer(autoPlay: false)
        player.boot()
        XCTAssertNil(player.engine!.state().pose, "not started")
        a.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0)])
        XCTAssertFalse(a.playing)
        XCTAssertEqual(events, [])
        // Values set before play change the first pose, and the frame shown.
        player.set("working", true)
        player.set("focus", true)
        XCTAssertEqual(b.clip, "work-loop")
        b.decodeFirstFrame()
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)])
        let before = b.log
        player.play()
        XCTAssertEqual(events, ["pose:working", "clip:work-loop"])
        XCTAssertEqual(b.log, before + ["restart 1.0"], "the frame on screen starts playing in place")
    }

    func testResetStartsAgainFromTheDefaults() {
        let player = started()
        player.set("working", true)
        end(a, after: 2)
        b.decodeFirstFrame()
        end(b, after: 1)
        events = []
        player.reset()
        XCTAssertEqual(events, ["pose:idle"])
        XCTAssertEqual(player.engine!.value(name: "working"), .bool(value: false))
    }

    func testPreviewCalls() {
        let player = started()
        player.preview.showPose("working")
        XCTAssertEqual(events, ["pose:working"])
        XCTAssertEqual(b.clip, "work-loop")
        b.decodeFirstFrame()
        player.preview.playMove("to-idle")
        XCTAssertEqual(player.engine!.state().clip, "to-idle")
        player.preview.showPose("nowhere")
        XCTAssertEqual(events.last, "error:unknown_pose")
    }

    // MARK: - Files and sizes

    func testSizeVariantsFollowTheMascotOnScreen() throws {
        var delivery = PlayerTests.delivery()
        var edges = delivery["runtimeGraph"]!["edges"]!.array!
        edges[0]["videos"] = .object([
            "hevc": .string("https://cdn.test/idle-loop.mov"),
            "variants": .object([
                "720": .object(["hevc": .string("https://cdn.test/idle-loop-720.mov")]),
                "360": .object(["hevc": .string("https://cdn.test/idle-loop-360.mov"), "webm": .string("x.webm")]),
                "180": .object(["webm": .string("https://cdn.test/idle-loop-180.webm")]),
            ]),
        ])
        delivery["runtimeGraph"]?["edges"] = .array(edges)
        let parsed = try Delivery(json: Data(delivery.text.utf8))
        XCTAssertEqual(parsed.file(edge: "idle-loop", pixels: 100)?.key, "idle-loop@360")
        XCTAssertEqual(parsed.file(edge: "idle-loop", pixels: 360)?.key, "idle-loop@360")
        XCTAssertEqual(parsed.file(edge: "idle-loop", pixels: 361)?.key, "idle-loop@720")
        XCTAssertEqual(parsed.file(edge: "idle-loop", pixels: 2000)?.key, "idle-loop", "else the base file")
        XCTAssertEqual(parsed.file(edge: "idle-loop", pixels: nil)?.key, "idle-loop")
        XCTAssertNil(parsed.file(edge: "go-work", pixels: 100), "no HEVC file")

        let player = makePlayer(delivery)
        player.displayPixels = 200 * 2
        player.boot()
        XCTAssertEqual(a.file?.lastPathComponent, "idle-loop-720.mov")
    }

    func testThePoseClipPicksItsSizeAgainAtItsNextLoopOnceTheViewHasOne() {
        var delivery = PlayerTests.delivery()
        var edges = delivery["runtimeGraph"]!["edges"]!.array!
        edges[0]["videos"] = .object([
            "hevc": .string("https://cdn.test/idle-loop.mov"),
            "variants": .object(["720": .object(["hevc": .string("https://cdn.test/idle-loop-720.mov")])]),
        ])
        delivery["runtimeGraph"]?["edges"] = .array(edges)
        let player = makePlayer(delivery)
        // The player starts before the view's first layout: no size yet.
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(a.file?.lastPathComponent, "idle-loop.mov")
        events = []
        player.displayPixels = 400
        XCTAssertEqual(b.log, ["load idle-loop-720.mov"], "the file for this size decodes ahead")
        b.decodeFirstFrame()
        end(a, after: 2)
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)], "the next loop swaps to it")
        XCTAssertEqual(events, ["clip:idle-loop"])
        XCTAssertEqual(player.engine!.state().shownAt, 2000)
        end(b, after: 2)
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)], "then it loops in place")
        XCTAssertEqual(b.log.last, "restart 1.0")
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
    }

    func testMediaFieldsOfTheWrongTypeAreIgnored() throws {
        var delivery = PlayerTests.delivery()
        delivery["deliveryAccess"] = .string("weird")
        var graph = delivery["runtimeGraph"]!
        var nodes = graph["nodes"]!.array!
        nodes[0]["transparentThumbnailUrl"] = .int(3)
        var edges = graph["edges"]!.array!
        edges[0]["sound"] = .object(["url": .string("https://cdn.test/hum.mp3"), "volume": .string("loud")])
        edges[1]["hasAudio"] = .string("yes")
        graph["nodes"] = .array(nodes)
        graph["edges"] = .array(edges)
        delivery["runtimeGraph"] = graph
        let parsed = try Delivery(json: Data(delivery.text.utf8))
        XCTAssertNil(parsed.stills["idle"])
        XCTAssertEqual(parsed.edges["idle-loop"]?.sound?.volume, 1)
        XCTAssertEqual(parsed.edges["work-loop"]?.hasAudio, false)
        XCTAssertNil(parsed.expiresAt)
        _ = started(delivery)
        XCTAssertEqual(stage.presented, [.slot(0)], "the engine accepts it, and so does the player")
    }

    func testAnEdgeWithoutAnHEVCFileFailsAtOnce() {
        var delivery = PlayerTests.delivery()
        var edges = delivery["runtimeGraph"]!["edges"]!.array!
        edges[2]["videos"] = .object(["webm": .string("https://cdn.test/to-work.webm")])
        delivery["runtimeGraph"]?["edges"] = .array(edges)
        let player = started(delivery)
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(events, ["error:clip_failed", "pose:working"])
    }

    // MARK: - Signed links

    func signedDelivery(_ host: String, expiresIn seconds: TimeInterval, from date: Date) -> JSON {
        var text = PlayerTests.deliveryText.replacingOccurrences(of: "https://cdn.test/", with: host)
        let expires = ISO8601DateFormatter().string(from: date.addingTimeInterval(seconds))
        text = text.replacingOccurrences(
            of: "\"mode\": \"selected\",",
            with: """
                "mode": "selected",
                "deliveryAccess": {"urlMode": "signed", "signedUrlExpiresAt": "\(expires)", "lastFetchedAt": "x"},
                """)
        return try! JSON(data: Data(text.utf8))
    }

    func testSignedLinksAreRefreshedBeforeTheyExpire() async throws {
        let start = FakeHost().date
        let player = makePlayer(signedDelivery("https://cdn.test/v1/", expiresIn: 3600, from: start))
        var asked = 0
        let fresh = signedDelivery("https://cdn.test/v2/", expiresIn: 7200, from: start)
        player.getDelivery = {
            asked += 1
            return Data(fresh.text.utf8)
        }
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(3600 - 5 * 60)])
        await player.refresh().value
        XCTAssertEqual(asked, 1)
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(7200 - 5 * 60)], "five minutes before")
        // The engine kept running; new files come from the new links, under
        // the same cache keys.
        XCTAssertEqual(player.engine!.state().clip, "idle-loop")
        player.set("working", true)
        let request = loader.requests.last!
        XCTAssertEqual(request.url.absoluteString, "https://cdn.test/v2/to-work.mov")
        XCTAssertEqual(request.key, "to-work")
    }

    func testAFailedRefreshIsReportedAndRetried() async throws {
        let start = FakeHost().date
        let player = makePlayer(signedDelivery("https://cdn.test/v1/", expiresIn: 3600, from: start))
        struct Offline: Error {}
        player.getDelivery = { throw Offline() }
        player.boot()
        events = []
        await player.refresh().value
        XCTAssertEqual(events, ["error:refresh_failed"])
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(30)], "tried again 30 s later")
    }

    func testLinksThatExpireSoonAreRefreshedHalfwayAndNotInALoop() async throws {
        let start = FakeHost().date
        let player = makePlayer(signedDelivery("https://cdn.test/v1/", expiresIn: 8 * 60, from: start))
        let same = signedDelivery("https://cdn.test/v2/", expiresIn: 8 * 60, from: start)
        player.getDelivery = { Data(same.text.utf8) }
        player.boot()
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(4 * 60)])
        await player.refresh().value
        XCTAssertEqual(events, [])
        XCTAssertEqual(
            host.wallTimers.map(\.date), [start.addingTimeInterval(30)], "links that last no longer: 30 s later")
    }

    func testUpdateDeliverySwapsTheLinksOfTheSameRelease() {
        let start = FakeHost().date
        let player = makePlayer(signedDelivery("https://cdn.test/v1/", expiresIn: 3600, from: start))
        loader.failing = ["to-work.mov"]
        player.boot()
        a.decodeFirstFrame()
        XCTAssertEqual(player.engine!.state().clip, "idle-loop")
        loader.failing = []
        let fresh = signedDelivery("https://cdn.test/v2/", expiresIn: 7200, from: start)
        XCTAssertTrue(player.updateDelivery(Data(fresh.text.utf8)))
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"], "no error, and the engine keeps running")
        // The clip that failed is tried again, from its new link.
        XCTAssertEqual(loader.requests.last?.url.absoluteString, "https://cdn.test/v2/to-work.mov")
        XCTAssertTrue(host.wallTimers.isEmpty, "no getDelivery: nothing to ask")
        player.set("working", true)
        end(a, after: 2)
        XCTAssertEqual(player.engine!.state().clip, "to-work", "playable again")
        XCTAssertEqual(b.file?.absoluteString, "https://cdn.test/v2/to-work.mov")
    }

    func testUpdateDeliveryRefusesAnotherReleaseAndAnUnreadableOne() {
        let player = started()
        var other = PlayerTests.delivery()
        var edges = other["runtimeGraph"]!["edges"]!.array!
        edges.removeLast()
        other["runtimeGraph"]?["edges"] = .array(edges)
        XCTAssertFalse(player.updateDelivery(Data(other.text.utf8)))
        var released = PlayerTests.delivery()
        released["release"] = .object(["id": .string("rel_2")])
        XCTAssertFalse(player.updateDelivery(Data(released.text.utf8)))
        XCTAssertFalse(player.updateDelivery(Data("{not json".utf8)))
        XCTAssertEqual(events, ["error:refresh_mismatch", "error:refresh_mismatch", "error:refresh_failed"])
        XCTAssertEqual(player.engine!.state().clip, "idle-loop")
    }

    func testCapabilitiesAreTheReleasesOwnUnchanged() {
        var delivery = PlayerTests.delivery()
        delivery["runtimeGraph"]?["capabilities"] = .object([
            "actions": .object([
                "nap": .object([
                    "name": .string("Nap"), "input": .string("action::nap"), "description": .string("Dozes off."),
                ])
            ])
        ])
        let player = makePlayer(delivery)
        XCTAssertEqual(
            player.capabilities,
            .object([
                "actions": .object([
                    "nap": .object([
                        "name": .string("Nap"), "input": .string("action::nap"),
                        "description": .string("Dozes off."),
                    ])
                ])
            ]))
        XCTAssertNil(makePlayer().capabilities, "a graph without capabilities")
        XCTAssertNil(makeClipPlayer().capabilities, "a lone clip")
    }

    func testPublicLinksAreNeverRefreshed() {
        let player = makePlayer()
        player.getDelivery = { Data() }
        player.boot()
        XCTAssertTrue(host.wallTimers.isEmpty)
    }

    func testDestroyStopsEverything() {
        let player = started()
        player.destroy()
        XCTAssertEqual(sounds.log.last, "stop all")
        XCTAssertEqual(host.pendingTimers, 0, "the wake is cancelled")
        player.set("working", true)
        XCTAssertNil(player.engine)
    }

    func testNothingStillLoadingShowsOrPlaysAfterDestroy() {
        let player = started()
        player.fire("nap")
        end(a, after: 2)
        b.decodeFirstFrame()
        loader.holding = true
        end(b, after: 1)
        // The napping still and the way back's file are on their way.
        XCTAssertEqual(player.engine!.state().clip, "still:napping")
        player.destroy()
        loader.release()
        XCTAssertEqual(stage.presented, [.slot(0), .slot(1)], "the still never shows")

        let other = started()
        loader.holding = true
        other.set("working", true)
        end(a, after: 2)
        // The move's clip and its sound are on their way.
        other.destroy()
        loader.release()
        XCTAssertEqual(sounds.log.last, "stop all", "the sound never plays")
        XCTAssertFalse(b.isLoading, "the clip never loads")
        XCTAssertEqual(stage.presented, [.slot(0)])
    }
}
