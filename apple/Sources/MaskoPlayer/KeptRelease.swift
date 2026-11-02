import CryptoKit
import Foundation

/// A canvas's latest release, read from its URL with an app key and kept on
/// the device (player.md, "A canvas from its URL"). Every file is downloaded
/// once and kept, named after its link's path: the path never changes, only
/// the signature does. `release.json` is the last release whose files were
/// all kept, so the mascot plays offline.
@MainActor
final class KeptRelease {
    /// How the release reaches the device; tests replace it.
    struct Network {
        var data: (URLRequest) async throws -> (Data, URLResponse)
        var download: (URL) async throws -> (URL, URLResponse)

        static let real = Network(
            data: { try await URLSession.shared.data(for: $0) },
            download: { try await URLSession.shared.download(from: $0) }
        )
    }

    struct Failure: Error, CustomStringConvertible {
        let description: String
    }

    /// The canvas URL, with the player's target and formats.
    let request: URL
    /// This canvas's kept files.
    let files: URL
    private let appKey: String
    private let folder: URL
    private let root: URL
    private let network: Network

    /// `Application Support/<bundle id>/MaskoPlayer`, left out of backups:
    /// everything in it can be downloaded again.
    nonisolated static var root: URL {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let app = Bundle.main.bundleIdentifier.map { support.appendingPathComponent($0, isDirectory: true) } ?? support
        return app.appendingPathComponent("MaskoPlayer", isDirectory: true)
    }

    init(url: URL, appKey: String, root: URL = KeptRelease.root, network: Network = .real) {
        request = KeptRelease.request(url)
        self.appKey = appKey
        self.root = root
        self.network = network
        var place = URLComponents(url: url, resolvingAgainstBaseURL: false)
        place?.query = nil
        folder = root.appendingPathComponent(KeptRelease.digest(place?.string ?? url.absoluteString, length: 16))
        files = folder.appendingPathComponent("files", isDirectory: true)
    }

    /// The URL with `target=apple` and the player's `formats`, unless it
    /// names them.
    static func request(_ url: URL) -> URL {
        guard var parts = URLComponents(url: url, resolvingAgainstBaseURL: false) else { return url }
        var items = parts.queryItems ?? []
        if !items.contains(where: { $0.name == "target" }) {
            items.append(URLQueryItem(name: "target", value: "apple"))
        }
        if !items.contains(where: { $0.name == "formats" }) {
            items.append(URLQueryItem(name: "formats", value: MaskoPlayer.formats))
        }
        parts.queryItems = items
        return parts.url ?? url
    }

    // MARK: - The kept release

    /// The kept release's delivery, when every file it plays is here.
    func kept() -> Data? {
        guard let data = try? Data(contentsOf: folder.appendingPathComponent("release.json")),
            let links = try? KeptRelease.links(in: data), links.allSatisfy(has)
        else { return nil }
        return data
    }

    /// The local file for a link, named after its path.
    func file(for link: URL) -> URL {
        files.appendingPathComponent(KeptRelease.name(link))
    }

    func has(_ link: URL) -> Bool {
        FileManager.default.fileExists(atPath: file(for: link).path)
    }

    /// The file name for a link: a digest of its path, with its extension.
    nonisolated static func name(_ link: URL) -> String {
        let suffix = link.pathExtension.isEmpty ? "bin" : link.pathExtension
        return digest(link.path, length: 64) + "." + suffix
    }

    /// Deletes kept files the kept release does not play. The player calls it
    /// before it plays anything.
    func removeUnused() {
        guard let data = try? Data(contentsOf: folder.appendingPathComponent("release.json")),
            let links = try? KeptRelease.links(in: data), !links.isEmpty,
            let names = try? FileManager.default.contentsOfDirectory(atPath: files.path)
        else { return }
        let used = Set(links.map(KeptRelease.name))
        for name in names where !used.contains(name) {
            try? FileManager.default.removeItem(at: files.appendingPathComponent(name))
        }
    }

    // MARK: - The latest release

    /// Asks the canvas URL for its latest release and returns its delivery.
    func latest() async throws -> Data {
        var ask = URLRequest(url: request, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 30)
        ask.setValue("Bearer \(appKey)", forHTTPHeaderField: "Authorization")
        ask.setValue(installId(), forHTTPHeaderField: "Masko-Install-Id")
        let (body, response) = try await network.data(ask)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        let answer = try? JSONSerialization.jsonObject(with: body) as? [String: Any]
        guard status == 200 else {
            let message = (answer?["error"] as? [String: Any])?["message"] as? String
            throw Failure(description: "the canvas URL answered \(status)" + (message.map { ": \($0)" } ?? ""))
        }
        guard let delivery = (answer?["data"] as? [String: Any])?["delivery"] as? [String: Any],
            let data = try? JSONSerialization.data(withJSONObject: delivery, options: [.withoutEscapingSlashes])
        else { throw Failure(description: "the canvas URL gave no delivery") }
        return data
    }

    /// Downloads every file of the delivery that is not here yet, then keeps
    /// it as the release to play, also after the next launch.
    func keep(_ delivery: Data) async throws {
        for link in try KeptRelease.links(in: delivery).sorted(by: { $0.path < $1.path }) where !has(link) {
            try await download(link)
        }
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try delivery.write(to: folder.appendingPathComponent("release.json"), options: .atomic)
    }

    private func download(_ link: URL) async throws {
        let (temporary, response) = try await network.download(link)
        defer { try? FileManager.default.removeItem(at: temporary) }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 200
        let size = (try? temporary.resourceValues(forKeys: [.fileSizeKey]).fileSize).flatMap { $0 } ?? 0
        let expected = response.expectedContentLength
        guard (200..<300).contains(status), size > 0, expected < 0 || Int64(size) == expected else {
            throw Failure(description: "\(link.lastPathComponent) arrived incomplete (\(status))")
        }
        try KeptRelease.place(temporary, at: file(for: link))
    }

    /// Moves a downloaded file into place: next to it first, then renamed, so
    /// a file is either complete or not there.
    nonisolated static func place(_ temporary: URL, at destination: URL) throws {
        let folder = destination.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let part = folder.appendingPathComponent(UUID().uuidString + ".part")
        try FileManager.default.moveItem(at: temporary, to: part)
        guard rename(part.path, destination.path) == 0 else {
            try? FileManager.default.removeItem(at: part)
            throw Failure(description: "cannot keep \(destination.lastPathComponent)")
        }
    }

    /// A random id this installed copy keeps, sent as `Masko-Install-Id` so
    /// the mascot's owner sees how many copies are active.
    private func installId() -> String {
        let file = root.appendingPathComponent("install-id")
        if let saved = try? String(contentsOf: file, encoding: .utf8), UUID(uuidString: saved) != nil {
            return saved
        }
        let id = UUID().uuidString.lowercased()
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var folder = root
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? folder.setResourceValues(values)
        try? Data(id.utf8).write(to: file, options: .atomic)
        return id
    }

    /// Every link of a delivery's graph: clips, stills and sounds.
    nonisolated static func links(in delivery: Data) throws -> Set<URL> {
        let root = try JSONSerialization.jsonObject(with: delivery) as? [String: Any]
        var links = Set<URL>()
        func walk(_ value: Any) {
            if let text = value as? String {
                if text.hasPrefix("https://") || text.hasPrefix("http://"), let link = URL(string: text) {
                    links.insert(link)
                }
            } else if let list = value as? [Any] {
                list.forEach(walk)
            } else if let object = value as? [String: Any] {
                object.values.forEach(walk)
            }
        }
        walk(root?["runtimeGraph"] ?? [:])
        return links
    }

    nonisolated private static func digest(_ text: String, length: Int) -> String {
        String(SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined().prefix(length))
    }
}
