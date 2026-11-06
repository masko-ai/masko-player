import Foundation
import XCTest

@testable import MaskoPlayer

/// A canvas read from its URL with an app key (player.md, "A canvas from its
/// URL"): the request, the kept files, offline play, and a newer release
/// taking over when the initial pose's loop starts again.
@MainActor
final class CanvasURLTests: XCTestCase {
    var root: URL!
    var network: FakeNetwork!
    var stage: FakeStage!
    var loader: FakeLoader!
    var sounds: FakeSounds!
    var host: FakeHost!
    var events: [String] = []

    var a: FakeSlot { stage.fakeSlots[0] }
    var b: FakeSlot { stage.fakeSlots[1] }

    static let url = URL(string: "https://api.masko.test/v1/canvases/c1/delivery")!

    override func setUp() async throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("masko-kept-\(UUID().uuidString)", isDirectory: true)
        network = FakeNetwork()
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: root)
    }

    /// A release whose files are named after it; `work` adds the move to the
    /// working pose.
    static func release(_ id: String, work: Bool = true) -> JSON {
        var edges: [String] = [
            """
            {"id": "idle-loop", "source": "idle", "target": "idle", "isLoop": true, "duration": 2,
             "videos": {"av1": "https://cdn.test/\(id)/idle-loop-\(id).mp4"}}
            """,
            """
            {"id": "work-loop", "source": "working", "target": "working", "isLoop": true, "duration": 2,
             "videos": {"av1": "https://cdn.test/\(id)/work-loop-\(id).mp4"}}
            """,
            """
            {"id": "to-idle", "source": "working", "target": "idle", "isLoop": false, "duration": 1,
             "conditions": [{"input": "behavior::working", "value": false}],
             "videos": {"av1": "https://cdn.test/\(id)/to-idle-\(id).mp4"}}
            """,
        ]
        if work {
            edges.append(
                """
                {"id": "to-work", "source": "idle", "target": "working", "isLoop": false, "duration": 1,
                 "conditions": [{"input": "behavior::working"}],
                 "videos": {"av1": "https://cdn.test/\(id)/to-work-\(id).mp4"}}
                """)
        }
        let text = """
            {
              "deliverySchemaVersion": 1, "playerContract": 1, "requires": [], "target": "apple",
              "mode": "selected", "release": {"id": "sha256:\(id)"},
              "runtimeGraph": {
                "runtimeGraphSchemaVersion": 1, "initialNode": "idle", "autoPlay": true,
                "nodes": [
                  {"id": "idle", "transparentThumbnailUrl": "https://cdn.test/\(id)/idle-\(id).png"},
                  {"id": "working", "transparentThumbnailUrl": "https://cdn.test/\(id)/working-\(id).png"}
                ],
                "inputs": [{"name": "behavior::working", "type": "boolean", "default": false}],
                "edges": [\(edges.joined(separator: ","))],
                "capabilities": {"actions": {"\(id)": {"input": "action::\(id)"}}}
              },
              "deliveryAccess": {"urlMode": "signed", "signedUrlExpiresAt": "2027-01-15T08:00:00.000Z"}
            }
            """
        return try! JSON(data: Data(text.utf8))
    }

    func kept(network: FakeNetwork? = nil) -> KeptRelease {
        KeptRelease(url: Self.url, appKey: "masko_app_test", root: root, network: (network ?? self.network).network)
    }

    func makePlayer() -> MaskoPlayer {
        stage = FakeStage()
        loader = FakeLoader()
        sounds = FakeSounds()
        host = FakeHost()
        events = []
        let player = MaskoPlayer(
            release: kept(), autoPlay: nil, stage: stage, loader: loader, sounds: sounds, host: host.host)
        player.onEvent = { [unowned self] event in
            switch event {
            case let .pose(p): self.events.append("pose:\(p)")
            case let .clip(c, _, _): self.events.append("clip:\(c)")
            case let .reactionDone(name, _): self.events.append("done:\(name)")
            case let .movement(edge, _, _, _): self.events.append("movement:\(edge)")
            case let .error(code, _): self.events.append("error:\(code)")
            }
        }
        return player
    }

    /// Boots the player and waits for its first check.
    func booted(_ player: MaskoPlayer) async {
        player.boot()
        await player.checking?.value
    }

    /// Boots a player whose release is kept, starts it, then lets its first
    /// check answer: what it brings comes while the mascot plays.
    func playing(_ player: MaskoPlayer) async {
        player.boot()
        a.decodeFirstFrame()
        await player.checking?.value
    }

    func end(_ slot: FakeSlot, after seconds: TimeInterval) {
        host.advance(seconds)
        slot.end()
    }

    // MARK: - The request and the kept files

    func testTheCanvasURLIsAskedWithTheAppKeyThePlatformAndAV1() async throws {
        network.latest = Self.release("r1")
        let release = kept()
        _ = try await release.latest()
        _ = try await release.latest()
        let asked = network.asked[0]
        let query = URLComponents(url: asked.url!, resolvingAgainstBaseURL: false)!.queryItems!
        XCTAssertEqual(query, [URLQueryItem(name: "target", value: "apple"), URLQueryItem(name: "formats", value: "av1")])
        XCTAssertEqual(asked.value(forHTTPHeaderField: "Authorization"), "Bearer masko_app_test")
        let install = try XCTUnwrap(asked.value(forHTTPHeaderField: "Masko-Install-Id"))
        XCTAssertNotNil(UUID(uuidString: install))
        XCTAssertEqual(network.asked[1].value(forHTTPHeaderField: "Masko-Install-Id"), install, "kept by this copy")
    }

    func testARefusalIsAFailureWithItsMessage() async {
        network.status = 401
        let release = kept()
        do {
            _ = try await release.latest()
            XCTFail("a 401 is a failure")
        } catch {
            XCTAssertEqual(String(describing: error), "the canvas URL answered 401: Invalid API key")
        }
    }

    func testEveryFileIsDownloadedOnceAndTheReleaseKeptOnlyWhenComplete() async throws {
        let release = kept()
        let delivery = Data(Self.release("r1").text.utf8)
        XCTAssertNil(release.kept())
        try await release.keep(delivery)
        XCTAssertEqual(network.downloaded.count, 6, "four clips and two stills")
        XCTAssertNotNil(release.kept())
        try await release.keep(delivery)
        XCTAssertEqual(network.downloaded.count, 6, "a file kept is never downloaded again")

        // A missing file: the release is not complete any more.
        let link = URL(string: "https://cdn.test/r1/to-work-r1.mp4")!
        try FileManager.default.removeItem(at: release.file(for: link))
        XCTAssertNil(release.kept())
    }

    func testAFileNamedAfterItsPathOutlivesItsSignature() {
        let signed = URL(string: "https://cdn.test/r1/idle-loop-r1.mp4?Expires=1&Signature=a")!
        let resigned = URL(string: "https://cdn.test/r1/idle-loop-r1.mp4?Expires=2&Signature=b")!
        XCTAssertEqual(KeptRelease.name(signed), KeptRelease.name(resigned))
        XCTAssertTrue(KeptRelease.name(signed).hasSuffix(".mp4"))
    }

    func testAFileThatArrivesIncompleteIsNotKept() async {
        network.shortBy = 1
        let release = kept()
        do {
            try await release.keep(Data(Self.release("r1").text.utf8))
            XCTFail("an incomplete file is a failure")
        } catch {}
        XCTAssertNil(release.kept())
        let names = (try? FileManager.default.contentsOfDirectory(atPath: release.files.path)) ?? []
        XCTAssertTrue(names.isEmpty, "nothing half there")
    }

    func testFilesOfOlderReleasesAreDeletedBeforeAnythingPlays() async throws {
        let release = kept()
        try await release.keep(Data(Self.release("r1").text.utf8))
        try await release.keep(Data(Self.release("r2").text.utf8))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: release.files.path).count, 12)
        release.removeUnused()
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: release.files.path).count, 6)
        XCTAssertNotNil(release.kept(), "the kept release keeps every file")
    }

    // MARK: - Playing

    func testTheFirstDeliveryPlaysAtOnceAndEveryFileIsKept() async {
        network.latest = Self.release("r1")
        let player = makePlayer()
        player.set("behavior::working", true)
        await booted(player)
        XCTAssertEqual(a.clip, "to-work-r1", "it plays from its links at once, and the value set before chose")
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:idle", "clip:to-work"])
        XCTAssertNotNil(kept().kept(), "every file kept")
        XCTAssertEqual(host.wallTimers.count, 1, "asked again in an hour")
        XCTAssertEqual(host.wallTimers[0].date, host.date.addingTimeInterval(3600))
    }

    func testAKeptReleasePlaysOffline() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        network.offline = true
        let player = makePlayer()
        await booted(player)
        XCTAssertEqual(a.clip, "idle-loop-r1")
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["error:refresh_failed", "pose:idle", "clip:idle-loop"])
        XCTAssertEqual(host.wallTimers.map(\.date), [host.date.addingTimeInterval(3600)], "it asks in an hour")
    }

    func testAFirstDeliveryThatDoesNotComeIsAskedForLessAndLessOften() async {
        network.offline = true
        let player = makePlayer()
        await booted(player)
        XCTAssertEqual(events, ["error:refresh_failed"])
        XCTAssertNil(player.engine)
        let start = host.date
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(30)])
        host.runWallTimer()
        await player.checking?.value
        XCTAssertEqual(host.wallTimers.map(\.date), [start.addingTimeInterval(30 + 60)])
        network.offline = false
        network.latest = Self.release("r1")
        host.runWallTimer()
        await player.checking?.value
        XCTAssertEqual(a.clip, "idle-loop-r1", "plays once it comes")
    }

    func testTheSameReleaseOnlyRenewsItsLinks() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        network.latest = Self.release("r1")
        let player = makePlayer()
        await booted(player)
        a.decodeFirstFrame()
        end(a, after: 2)
        XCTAssertEqual(a.log, ["load idle-loop-r1.mp4", "restart 1.0", "restart 1.0"], "no switch")
        XCTAssertEqual(network.downloaded.count, 6, "nothing more to download")
    }

    func testANewerReleaseBeforeTheStartGivesTheFirstFrame() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        network.latest = Self.release("r2")
        let player = makePlayer()
        await booted(player)
        XCTAssertEqual(a.clip, "idle-loop-r2")
        a.decodeFirstFrame()
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"])
        XCTAssertEqual(player.engine!.state().pose, "idle")
    }

    func testANewerReleaseTakesOverWhenTheInitialLoopStartsAgain() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        network.latest = Self.release("r2")
        let player = makePlayer()
        await playing(player)
        XCTAssertNotNil(kept().kept())
        XCTAssertEqual(network.downloaded.count, 12, "every file of r2 is kept before it plays")
        XCTAssertEqual(
            player.capabilities,
            .object(["actions": .object(["r1": .object(["input": .string("action::r1")])])]),
            "r2 waits for the initial loop: r1 is still on screen")
        events = []
        end(a, after: 2)
        // The old loop's last frame stays on screen until the new loop's first
        // frame is decoded in the other slot.
        XCTAssertEqual(a.log.last, "restart 1.0", "the old loop is not started again")
        XCTAssertEqual(b.log, ["load idle-loop-r2.mp4"])
        XCTAssertEqual(stage.showing, .slot(0))
        b.decodeFirstFrame()
        XCTAssertEqual(stage.showing, .slot(1))
        XCTAssertEqual(stage.swapsWithoutFrame, 0)
        XCTAssertEqual(events, ["pose:idle", "clip:idle-loop"])
        XCTAssertEqual(player.engine!.state().clock, 0, "the new release starts at play time 0")
        XCTAssertEqual(
            player.capabilities,
            .object(["actions": .object(["r2": .object(["input": .string("action::r2")])])]),
            "what the mascot on screen can do is r2's")
        // It plays on from r2's files.
        player.set("behavior::working", true)
        end(b, after: 2)
        XCTAssertEqual(a.clip, "to-work-r2")
    }

    func testANewerReleaseWaitsUntilTheMascotIsBackOnItsInitialPose() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        network.latest = Self.release("r2")
        let player = makePlayer()
        player.set("behavior::working", true)
        await playing(player)
        XCTAssertEqual(a.clip, "to-work-r1")
        end(a, after: 1)
        XCTAssertEqual(b.clip, "work-loop-r1")
        b.decodeFirstFrame()
        end(b, after: 2)
        XCTAssertEqual(b.log.last, "restart 1.0", "the working loop goes on in r1")
        player.set("behavior::working", false)
        end(b, after: 2)
        XCTAssertEqual(a.clip, "to-idle-r1")
        a.decodeFirstFrame()
        end(a, after: 1)
        XCTAssertEqual(b.clip, "idle-loop-r1", "back on the initial pose, still r1")
        b.decodeFirstFrame()
        end(b, after: 2)
        XCTAssertEqual(a.clip, "idle-loop-r2", "its loop starting again is the switch")
    }

    func testValuesTheAppSetCarryOverToTheNewerRelease() async throws {
        // r1 has no move to the working pose: working changes nothing there.
        try await kept().keep(Data(Self.release("r1", work: false).text.utf8))
        network.latest = Self.release("r2")
        let player = makePlayer()
        await playing(player)
        player.set("behavior::working", true)
        end(a, after: 2)
        XCTAssertEqual(b.clip, "idle-loop-r2", "r1 stayed on its initial pose: the switch")
        b.decodeFirstFrame()
        end(b, after: 2)
        XCTAssertEqual(a.clip, "to-work-r2", "working is still set, and r2 moves for it")
    }

    func testAReleaseTheEngineRefusesIsReportedAndTheOnePlayingGoesOn() async throws {
        try await kept().keep(Data(Self.release("r1").text.utf8))
        var refused = Self.release("r2")
        refused["playerContract"] = .int(99)
        network.latest = refused
        let player = makePlayer()
        await playing(player)
        XCTAssertEqual(events.last?.hasPrefix("error:"), true, "reported when it comes")
        XCTAssertEqual(network.downloaded.count, 6, "and never downloaded")
        end(a, after: 2)
        XCTAssertEqual(a.log, ["load idle-loop-r1.mp4", "restart 1.0", "restart 1.0"], "r1 goes on")
        XCTAssertEqual(player.engine!.state().clip, "idle-loop")
    }

    func testDestroyStopsAskingAndKeepsTheFiles() async throws {
        network.latest = Self.release("r1")
        let player = makePlayer()
        await booted(player)
        player.destroy()
        XCTAssertNotNil(kept().kept())
    }
}

/// The canvas URL and the file links, answered in memory.
@MainActor
final class FakeNetwork {
    var latest: JSON?
    var status = 200
    var offline = false
    /// Files arrive this many bytes short of their Content-Length.
    var shortBy = 0
    private(set) var asked: [URLRequest] = []
    private(set) var downloaded: [URL] = []

    struct Offline: Error {}

    var network: KeptRelease.Network {
        KeptRelease.Network(
            data: { [unowned self] request in
                try await MainActor.run {
                    self.asked.append(request)
                    if self.offline { throw Offline() }
                    let response = HTTPURLResponse(
                        url: request.url!, statusCode: self.status, httpVersion: nil, headerFields: nil)!
                    if self.status != 200 {
                        return (Data(#"{"error":{"message":"Invalid API key"}}"#.utf8), response)
                    }
                    let body = JSON.object(["data": .object(["delivery": self.latest ?? .null])])
                    return (Data(body.text.utf8), response)
                }
            },
            download: { [unowned self] link in
                try await MainActor.run {
                    if self.offline { throw Offline() }
                    self.downloaded.append(link)
                    let bytes = Data(link.absoluteString.utf8)
                    let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                    try bytes.write(to: file)
                    let response = HTTPURLResponse(
                        url: link, statusCode: 200, httpVersion: nil,
                        headerFields: ["Content-Length": "\(bytes.count + self.shortBy)"])!
                    return (file, response)
                }
            }
        )
    }
}
