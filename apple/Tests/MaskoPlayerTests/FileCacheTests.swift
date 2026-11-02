import Foundation
import XCTest

@testable import MaskoPlayer

/// Serves a few bytes for any URL, 404 for paths with "missing", never
/// answers paths with "slow", and counts the requests and the cancelled ones.
final class StubServer: URLProtocol {
    private static let lock = NSLock()
    private static var _hits: [String] = []
    private static var _stopped: [String] = []

    static var hits: [String] {
        lock.lock()
        defer { lock.unlock() }
        return _hits
    }

    static var stopped: [String] {
        lock.lock()
        defer { lock.unlock() }
        return _stopped
    }

    static func reset() {
        lock.lock()
        _hits = []
        _stopped = []
        lock.unlock()
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let url = request.url!
        Self.lock.lock()
        Self._hits.append(url.absoluteString)
        Self.lock.unlock()
        if url.path.contains("slow") { return }
        let status = url.path.contains("missing") ? 404 : 200
        let response = HTTPURLResponse(url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("bytes of \(url.path)".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {
        Self.lock.lock()
        Self._stopped.append(request.url!.absoluteString)
        Self.lock.unlock()
    }
}

@MainActor
final class FileCacheTests: XCTestCase {
    func makeCache(capacity: Int = 12) -> FileCache {
        StubServer.reset()
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubServer.self]
        return FileCache(session: URLSession(configuration: config), capacity: capacity)
    }

    func fetch(_ cache: FileCache, _ url: String, key: String) async -> URL? {
        await withCheckedContinuation { done in
            cache.file(URL(string: url)!, key: key) { done.resume(returning: $0) }
        }
    }

    func testAFileIsDownloadedOnceAndOutlivesItsLink() async throws {
        let cache = makeCache()
        defer { cache.clear() }
        let first = await fetch(cache, "https://cdn.test/a.mov?sig=1", key: "a")
        let file = try XCTUnwrap(first)
        XCTAssertEqual(try String(contentsOf: file, encoding: .utf8), "bytes of /a.mov")
        // A refreshed link to the same file: the cached copy plays.
        let again = await fetch(cache, "https://cdn.test/a.mov?sig=2", key: "a")
        XCTAssertEqual(again, file)
        XCTAssertEqual(StubServer.hits, ["https://cdn.test/a.mov?sig=1"])
    }

    func testRequestsForTheSameFileShareOneDownload() async throws {
        let cache = makeCache()
        defer { cache.clear() }
        async let one = fetch(cache, "https://cdn.test/b.mov", key: "b")
        async let two = fetch(cache, "https://cdn.test/b.mov", key: "b")
        let (x, y) = await (one, two)
        XCTAssertNotNil(x)
        XCTAssertEqual(x, y)
        XCTAssertEqual(StubServer.hits.count, 1)
    }

    func testAFailedDownloadGivesNoFile() async {
        let cache = makeCache()
        defer { cache.clear() }
        let file = await fetch(cache, "https://cdn.test/missing.mov", key: "m")
        XCTAssertNil(file)
    }

    func testOnlyTheMostRecentFilesAreKept() async throws {
        let cache = makeCache(capacity: 2)
        defer { cache.clear() }
        let fetched = await fetch(cache, "https://cdn.test/a.mov", key: "a")
        let a = try XCTUnwrap(fetched)
        _ = await fetch(cache, "https://cdn.test/b.mov", key: "b")
        _ = await fetch(cache, "https://cdn.test/a.mov", key: "a")  // a is recent again
        _ = await fetch(cache, "https://cdn.test/c.mov", key: "c")  // b goes
        XCTAssertTrue(FileManager.default.fileExists(atPath: a.path))
        _ = await fetch(cache, "https://cdn.test/b.mov", key: "b")
        XCTAssertEqual(StubServer.hits.filter { $0.hasSuffix("b.mov") }.count, 2)
    }

    /// Waits until `done` is true, for at most two seconds.
    func waitUntil(_ done: @escaping () -> Bool) async throws {
        var tries = 0
        while !done(), tries < 200 {
            tries += 1
            try await Task.sleep(nanoseconds: 10_000_000)
        }
    }

    func testClearCancelsTheDownloadsInFlight() async throws {
        let cache = makeCache()
        var answered = false
        cache.file(URL(string: "https://cdn.test/slow.mov")!, key: "s") { _ in answered = true }
        try await waitUntil { !StubServer.hits.isEmpty }
        XCTAssertEqual(StubServer.hits, ["https://cdn.test/slow.mov"])
        cache.clear()
        try await waitUntil { !StubServer.stopped.isEmpty }
        XCTAssertEqual(StubServer.stopped, ["https://cdn.test/slow.mov"], "the download is cancelled")
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertFalse(answered)
        // A cleared cache gives nothing more.
        cache.file(URL(string: "https://cdn.test/a.mov")!, key: "a") { _ in answered = true }
        try await Task.sleep(nanoseconds: 50_000_000)
        XCTAssertFalse(answered)
        XCTAssertEqual(StubServer.hits.count, 1)
    }

    func testDownloadsGoInTheFolderOfThisProcess() {
        XCTAssertEqual(DownloadFolder.current.deletingLastPathComponent(), DownloadFolder.root)
        XCTAssertEqual(DownloadFolder.root.lastPathComponent, "MaskoPlayer")
        XCTAssertEqual(
            DownloadFolder.root.deletingLastPathComponent().standardizedFileURL,
            FileManager.default.temporaryDirectory.standardizedFileURL)
    }

    func testFoldersThatEarlierLaunchesLeftAreDeleted() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("MaskoPlayerSweep-\(UUID().uuidString)")
        defer { try? files.removeItem(at: root) }
        // A running process holds its folder's lock: this one.
        let running = DownloadFolder.claim(in: root)
        try Data("x".utf8).write(to: running.appendingPathComponent("clip.mov"))
        // A process that ended left its folder and its lock file.
        let ended = root.appendingPathComponent(UUID().uuidString)
        try files.createDirectory(at: ended, withIntermediateDirectories: true)
        try Data("x".utf8).write(to: ended.appendingPathComponent("clip.mov"))
        try Data().write(to: root.appendingPathComponent(ended.lastPathComponent + ".lock"))
        // An older folder with no lock at all.
        let older = root.appendingPathComponent(UUID().uuidString)
        try files.createDirectory(at: older, withIntermediateDirectories: true)

        DownloadFolder.sweep(root)
        XCTAssertTrue(files.fileExists(atPath: running.appendingPathComponent("clip.mov").path))
        XCTAssertEqual(
            try files.contentsOfDirectory(atPath: root.path).sorted(),
            [running.lastPathComponent, running.lastPathComponent + ".lock"].sorted())
    }

    func testLocalFilesAreUsedWhereTheyAre() async throws {
        let cache = makeCache()
        let local = FileManager.default.temporaryDirectory.appendingPathComponent("masko-local-\(UUID()).mov")
        try Data("x".utf8).write(to: local)
        defer { try? FileManager.default.removeItem(at: local) }
        let got = await withCheckedContinuation { done in
            cache.file(local, key: "l") { done.resume(returning: $0) }
        }
        XCTAssertEqual(got, local)
        XCTAssertTrue(StubServer.hits.isEmpty)
    }

    // MARK: - Kept files (a canvas read from its URL)

    func makeKept() -> (FileCache, URL) {
        StubServer.reset()
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubServer.self]
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("masko-kept-cache-\(UUID().uuidString)", isDirectory: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: folder) }
        return (FileCache(keptIn: folder, session: URLSession(configuration: config)), folder)
    }

    func testAKeptFileIsNamedAfterItsPathAndOutlivesTheCache() async throws {
        let (cache, folder) = makeKept()
        let first = await fetch(cache, "https://cdn.test/r1/idle.mp4?Signature=a", key: "idle-loop")
        let local = try XCTUnwrap(first)
        let name = KeptRelease.name(URL(string: "https://cdn.test/r1/idle.mp4")!)
        XCTAssertEqual(local, folder.appendingPathComponent(name))
        XCTAssertEqual(String(decoding: try Data(contentsOf: local), as: UTF8.self), "bytes of /r1/idle.mp4")
        cache.clear()
        XCTAssertTrue(FileManager.default.fileExists(atPath: local.path), "kept files stay")

        // Another cache over the same folder, another signature: no download.
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [StubServer.self]
        let again = FileCache(keptIn: folder, session: URLSession(configuration: config))
        StubServer.reset()
        let second = await fetch(again, "https://cdn.test/r1/idle.mp4?Signature=b", key: "other-key")
        XCTAssertEqual(second, local)
        XCTAssertEqual(StubServer.hits, [])
    }

    func testAKeptFileThatFailsIsNotKept() async {
        let (cache, folder) = makeKept()
        let local = await fetch(cache, "https://cdn.test/r1/missing.mp4", key: "gone")
        XCTAssertNil(local)
        XCTAssertEqual((try? FileManager.default.contentsOfDirectory(atPath: folder.path)) ?? [], [])
    }
}
