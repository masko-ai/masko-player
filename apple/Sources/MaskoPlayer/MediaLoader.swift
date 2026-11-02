import AVFoundation
import CoreGraphics
import Foundation
import ImageIO

/// Gets a local copy of a clip, sound or still. `key` names what the file is
/// (an edge and its size, a pose's still), so a file fetched once keeps
/// playing after its signed link is refreshed.
@MainActor
protocol MediaLoader: AnyObject {
    func file(_ url: URL, key: String, done: @escaping @MainActor (URL?) -> Void)
    func image(_ url: URL, key: String, done: @escaping @MainActor (CGImage?) -> Void)
    /// A lone clip's length and whether it has a sound track, read from its
    /// local copy; nil when it cannot be read.
    func clipFile(_ url: URL, key: String, done: @escaping @MainActor (ClipFile?) -> Void)
}

/// What a lone clip's file holds: its length in seconds, and whether it has a
/// sound track (player.md, "One clip").
struct ClipFile: Equatable {
    var seconds: TimeInterval
    var voiced: Bool
}

/// Downloads files into a temporary folder and keeps the most recent few
/// (the preload cache of player.md), or, for a canvas read from its URL, into
/// its kept files, for good (`KeptRelease`). Local files are used where they
/// are.
@MainActor
final class FileCache: MediaLoader {
    private let session: URLSession
    private let folder: URL
    private let capacity: Int
    /// The kept files of a canvas read from its URL: named after each link's
    /// path, never evicted nor deleted.
    private let kept: URL?
    /// Keys from least to most recently used.
    private var order: [String] = []
    private var files: [String: URL] = [:]
    private var images: [String: CGImage] = [:]
    private var waiting: [String: [@MainActor (URL?) -> Void]] = [:]
    private var downloads: [String: URLSessionTask] = [:]
    private var closed = false

    init(session: URLSession = .shared, capacity: Int = 16) {
        self.session = session
        self.capacity = capacity
        folder = DownloadFolder.current.appendingPathComponent(UUID().uuidString, isDirectory: true)
        kept = nil
    }

    init(keptIn files: URL, session: URLSession = .shared) {
        self.session = session
        capacity = .max
        folder = files
        kept = files
    }

    deinit {
        if kept == nil { try? FileManager.default.removeItem(at: folder) }
    }

    func file(_ url: URL, key: String, done: @escaping @MainActor (URL?) -> Void) {
        if url.isFileURL {
            done(FileManager.default.fileExists(atPath: url.path) ? url : nil)
            return
        }
        guard !closed else { return }
        // A kept file is named after its link's path, whichever key asks.
        let key = kept == nil ? key : KeptRelease.name(url)
        if let local = files[key] {
            touch(key)
            done(local)
            return
        }
        if let kept, FileManager.default.fileExists(atPath: kept.appendingPathComponent(key).path) {
            done(kept.appendingPathComponent(key))
            return
        }
        if waiting[key] != nil {
            waiting[key]?.append(done)
            return
        }
        waiting[key] = [done]
        let destination =
            kept?.appendingPathComponent(key)
            ?? folder.appendingPathComponent(UUID().uuidString + "-" + url.lastPathComponent)
        let folder = self.folder
        let keep = kept != nil
        let task = session.downloadTask(with: url) { temporary, response, _ in
            let status = (response as? HTTPURLResponse)?.statusCode ?? 200
            let saved: URL? = {
                guard let temporary, (200..<300).contains(status) else { return nil }
                do {
                    if keep {
                        let size = (try? temporary.resourceValues(forKeys: [.fileSizeKey]).fileSize).flatMap { $0 } ?? 0
                        let expected = response?.expectedContentLength ?? -1
                        guard size > 0, expected < 0 || Int64(size) == expected else { return nil }
                        try KeptRelease.place(temporary, at: destination)
                        return destination
                    }
                    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                    try FileManager.default.moveItem(at: temporary, to: destination)
                    return destination
                } catch {
                    return nil
                }
            }()
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self.finish(key, saved) }
            }
        }
        downloads[key] = task
        task.resume()
    }

    private func finish(_ key: String, _ file: URL?) {
        downloads[key] = nil
        if closed {
            if let file, kept == nil { try? FileManager.default.removeItem(at: file) }
            return
        }
        if let file, kept == nil {
            files[key] = file
            touch(key)
            evict()
        }
        let callbacks = waiting.removeValue(forKey: key) ?? []
        callbacks.forEach { $0(file) }
    }

    func image(_ url: URL, key: String, done: @escaping @MainActor (CGImage?) -> Void) {
        let key = kept == nil ? key : KeptRelease.name(url)
        if let image = images[key] {
            done(image)
            return
        }
        file(url, key: key) { [weak self] local in
            guard let local else { return done(nil) }
            Task { @MainActor [weak self] in
                let image = await FileCache.decode(local)
                // A still decoded after `clear` never shows.
                guard let self, !self.closed else { return }
                if let image { self.images[key] = image }
                done(image)
            }
        }
    }

    func clipFile(_ url: URL, key: String, done: @escaping @MainActor (ClipFile?) -> Void) {
        file(url, key: key) { [weak self] local in
            guard let local else { return done(nil) }
            Task { @MainActor [weak self] in
                let asset = AVURLAsset(url: local)
                let seconds = try? await asset.load(.duration).seconds
                let voices = (try? await asset.loadTracks(withMediaType: .audio)) ?? []
                guard let self, !self.closed else { return }
                guard let seconds, seconds.isFinite, seconds > 0 else { return done(nil) }
                done(ClipFile(seconds: seconds, voiced: !voices.isEmpty))
            }
        }
    }

    /// Decodes an image file off the main thread.
    private nonisolated static func decode(_ file: URL) async -> CGImage? {
        CGImageSourceCreateWithURL(file as CFURL, nil).flatMap { CGImageSourceCreateImageAtIndex($0, 0, nil) }
    }

    private func touch(_ key: String) {
        order.removeAll { $0 == key }
        order.append(key)
    }

    private func evict() {
        while order.count > capacity {
            let key = order.removeFirst()
            if let file = files.removeValue(forKey: key) {
                try? FileManager.default.removeItem(at: file)
            }
        }
    }

    /// Cancels the downloads in flight and deletes every downloaded file, but
    /// kept files. The cache gives nothing afterwards.
    func clear() {
        closed = true
        downloads.values.forEach { $0.cancel() }
        downloads = [:]
        waiting = [:]
        files = [:]
        order = []
        images = [:]
        if kept == nil { try? FileManager.default.removeItem(at: folder) }
    }
}

/// Where this process downloads: `tmp/MaskoPlayer/<id>/`, held by a lock on
/// `tmp/MaskoPlayer/<id>.lock` while the process runs. An app that is killed
/// never deletes its files, so the first use deletes the folders earlier
/// processes left there, and keeps those another running process holds (two
/// Mac apps can share one temporary folder).
enum DownloadFolder {
    static let root = FileManager.default.temporaryDirectory.appendingPathComponent("MaskoPlayer", isDirectory: true)

    /// This process's folder. Making it starts the sweep, in the background.
    static let current: URL = {
        let root = DownloadFolder.root
        let folder = claim(in: root)
        DispatchQueue.global(qos: .utility).async { sweep(root) }
        return folder
    }()

    /// Makes a folder in `root` and holds its lock until the process ends.
    static func claim(in root: URL) -> URL {
        let id = UUID().uuidString
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        // The lock comes first and the descriptor is never closed, so a sweep
        // never sees this folder without its owner.
        _ = open(root.appendingPathComponent(id + ".lock").path, O_RDWR | O_CREAT | O_EXLOCK | O_NONBLOCK, 0o600)
        let folder = root.appendingPathComponent(id, isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder
    }

    /// Deletes every folder in `root` (and its lock) that no running process
    /// holds.
    static func sweep(_ root: URL) {
        guard let names = try? FileManager.default.contentsOfDirectory(atPath: root.path) else { return }
        let ids = Set(names.map { $0.hasSuffix(".lock") ? String($0.dropLast(5)) : $0 })
        for id in ids {
            let lock = root.appendingPathComponent(id + ".lock")
            let fd = open(lock.path, O_RDONLY | O_EXLOCK | O_NONBLOCK)
            if fd < 0 && errno != ENOENT {
                continue  // held by a running process
            }
            try? FileManager.default.removeItem(at: root.appendingPathComponent(id))
            try? FileManager.default.removeItem(at: lock)
            if fd >= 0 { close(fd) }
        }
    }
}
