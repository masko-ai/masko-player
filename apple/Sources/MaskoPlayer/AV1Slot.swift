import AVFoundation
#if os(macOS)
import CoreAudio
#endif
import Dav1d
import Metal
import QuartzCore
import VideoToolbox

/// A slot for stacked AV1 files: colour above, transparency below
/// (player.md, "The file and the size"). Apple's own player cannot show
/// them, so the slot reads the file's frames itself, decodes them with the
/// device's AV1 decoder when it has one and with dav1d otherwise, and draws
/// each one transparent with a Metal shader. The sound plays from the same
/// file, and the picture waits for it: the clock starts when the sound
/// reaches the speakers, so a voice stays on the lips, also on Bluetooth.
@MainActor
final class AV1Slot: VideoSlot {
    let layer = CAMetalLayer()
    var onEnd: (@MainActor () -> Void)?
    var onFail: (@MainActor () -> Void)?

    private(set) var file: URL?
    private var clip: AV1Clip?
    private var decoder: AV1Decoder?
    private var audio: SlotAudio?
    private var ready: (@MainActor (Bool) -> Void)?
    /// Bumped by every load, so a late read or first frame of an older load
    /// is ignored.
    private var loadID = 0
    /// The loaded file's first frame is drawn in the layer.
    private(set) var firstFrameShown = false
    private var played = false
    private var muted = true

    /// Seconds into the file when the clock last started or stopped.
    private var baseTime = 0.0
    private var startedAt: CFTimeInterval?
    private var rate = 1.0
    private var shown = -1
    private var ticker: Timer?
    /// How many frames the decoder keeps ready ahead of the one on screen.
    private let lookahead = 3

    init() {
        layer.device = StackedRenderer.shared?.device
        layer.pixelFormat = .bgra8Unorm
        layer.isOpaque = false
        layer.framebufferOnly = true
        layer.contentsGravity = .resizeAspect
    }

    var isMuted: Bool {
        get { muted }
        set {
            muted = newValue
            if newValue {
                audio?.stop()
            } else if startedAt != nil, let audio, let clip, audio.prepare() {
                _ = audio.play(from: clipTime + audio.delay * rate + clip.soundStart, rate: rate)
            }
        }
    }

    var clipTime: Double {
        guard let startedAt else { return baseTime }
        // Before the sound is heard, the clock holds the frame on screen.
        let t = baseTime + max(0, CACurrentMediaTime() - startedAt) * rate
        return min(t, clip?.duration ?? t)
    }

    func load(_ file: URL, ready: @escaping @MainActor (Bool) -> Void) {
        pause()
        if file == self.file, !played {
            if firstFrameShown {
                // Loaded ahead and still on its first frame.
                ready(true)
            } else {
                self.ready = ready
            }
            return
        }
        unload()
        self.file = file
        self.ready = ready
        loadID += 1
        let id = loadID
        Task { @MainActor [weak self] in
            let read = try? await AV1Clip.read(file)
            guard let self, self.loadID == id else { return }
            guard let read, let renderer = StackedRenderer.shared,
                let decoder = AV1Decoder(clip: read, device: renderer.device)
            else { return self.finishLoad(false) }
            self.clip = read
            self.decoder = decoder
            self.audio = read.sound.map { SlotAudio(file: $0) }
            self.layer.drawableSize = CGSize(width: read.width, height: read.height / 2)
            decoder.want(upTo: self.lookahead)
            self.waitForFirstFrame(id)
        }
    }

    /// Draws frame 0 as soon as it is decoded, then reports the load ready.
    private func waitForFirstFrame(_ id: Int, since: Date = Date()) {
        guard loadID == id, let decoder else { return }
        if let (_, frame) = decoder.take(upTo: 0, after: -1) {
            StackedRenderer.shared?.draw(frame, into: layer) { [weak self] ok in
                guard let self, self.loadID == id else { return }
                self.shown = 0
                self.firstFrameShown = ok
                self.finishLoad(ok)
            }
        } else if decoder.failed || Date().timeIntervalSince(since) > 10 {
            finishLoad(false)
        } else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.004) { [weak self] in
                MainActor.assumeIsolated { self?.waitForFirstFrame(id, since: since) }
            }
        }
    }

    private func finishLoad(_ ok: Bool) {
        if !ok {
            let done = ready
            unload()
            done?(false)
            return
        }
        let done = ready
        ready = nil
        done?(true)
    }

    func play(rate: Double) {
        guard let clip else { return }
        played = true
        baseTime = clipTime
        self.rate = rate
        startedAt = CACurrentMediaTime()
        // The picture starts when the sound is heard, not when it is asked
        // for: starting the sound and the speakers' own delay (much longer on
        // Bluetooth) would otherwise put the voice behind the lips.
        if !muted, let heard = audio?.play(from: baseTime + clip.soundStart, rate: rate) {
            startedAt = heard
        }
        startTicking()
    }

    func restart(rate: Double) {
        guard let decoder else { return }
        stopClock()
        audio?.stop()
        baseTime = 0
        shown = -1
        decoder.reset()
        decoder.want(upTo: lookahead)
        play(rate: rate)
    }

    func pause() {
        guard startedAt != nil else { return }
        baseTime = clipTime
        stopClock()
        audio?.pause()
    }

    /// Lets go of the file.
    func unload() {
        stopClock()
        audio?.stop()
        audio = nil
        decoder = nil
        clip = nil
        file = nil
        ready = nil
        loadID += 1
        firstFrameShown = false
        played = false
        baseTime = 0
        shown = -1
    }

    private func stopClock() {
        startedAt = nil
        ticker?.invalidate()
        ticker = nil
    }

    private func startTicking() {
        guard ticker == nil else { return }
        let timer = Timer(timeInterval: 1.0 / 120, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        RunLoop.main.add(timer, forMode: .common)
        ticker = timer
    }

    /// Shows the frame for the clock's time, and ends the clip at its length.
    private func tick() {
        guard let clip, let decoder else { return }
        if decoder.failed {
            stopClock()
            onFail?()
            return
        }
        let time = clipTime
        let due = clip.frameIndex(at: time)
        // The latest decoded frame that is due: a decoder running late skips
        // ahead instead of slowing the clip down.
        if due > shown, let (index, frame) = decoder.take(upTo: due, after: shown) {
            shown = index
            StackedRenderer.shared?.draw(frame, into: layer) { _ in }
        }
        decoder.want(upTo: due + lookahead)
        if time >= clip.duration, shown >= clip.times.count - 1 {
            baseTime = clip.duration
            stopClock()
            onEnd?()
        }
    }
}

// MARK: - The file

/// A stacked AV1 file read into memory: its compressed frames in order, when
/// each one shows, and its sound.
struct AV1Clip: @unchecked Sendable {
    let frames: [Data]
    /// When each frame shows, in seconds, in decoding order (AV1 in MP4 shows
    /// its frames in that order).
    let times: [Double]
    let duration: Double
    let format: CMFormatDescription
    let width: Int
    let height: Int
    let sound: AVAudioFile?
    /// Seconds into the sound when the first frame shows: frame times start at
    /// 0, the sound keeps the file's own offset to the picture.
    var soundStart = 0.0

    /// The last frame due at `time`.
    func frameIndex(at time: Double) -> Int {
        var low = 0
        var high = times.count - 1
        while low < high {
            let mid = (low + high + 1) / 2
            if times[mid] <= time + 0.0005 {
                low = mid
            } else {
                high = mid - 1
            }
        }
        return low
    }

    /// AV1's code in a sample description, `av01`.
    static let codec: FourCharCode = 0x6176_3031

    /// Reads the file's AV1 frames without decoding them, off the main thread.
    static func read(_ url: URL) async throws -> AV1Clip {
        let asset = AVURLAsset(url: url)
        guard let track = try await asset.loadTracks(withMediaType: .video).first,
            let format = try await track.load(.formatDescriptions).first,
            CMFormatDescriptionGetMediaSubType(format) == codec
        else { throw AV1Error.notAV1 }
        let length = try await track.load(.timeRange).duration.seconds
        let voiced = try await !asset.loadTracks(withMediaType: .audio).isEmpty
        let reader = try AVAssetReader(asset: asset)
        let output = AVAssetReaderTrackOutput(track: track, outputSettings: nil)
        reader.add(output)
        guard reader.startReading() else { throw AV1Error.unreadable }
        var frames: [Data] = []
        var times: [Double] = []
        while let sample = output.copyNextSampleBuffer() {
            guard let block = CMSampleBufferGetDataBuffer(sample) else { continue }
            var data = Data(count: CMBlockBufferGetDataLength(block))
            let status = data.withUnsafeMutableBytes { bytes in
                CMBlockBufferCopyDataBytes(block, atOffset: 0, dataLength: bytes.count, destination: bytes.baseAddress!)
            }
            guard status == noErr else { throw AV1Error.unreadable }
            frames.append(data)
            times.append(CMSampleBufferGetPresentationTimeStamp(sample).seconds)
        }
        guard reader.status == .completed, !frames.isEmpty else { throw AV1Error.unreadable }
        let start = times[0]
        times = times.map { $0 - start }
        let dimensions = CMVideoFormatDescriptionGetDimensions(format)
        var soundStart = 0.0
        if voiced, let sound = try await asset.loadTracks(withMediaType: .audio).first {
            soundStart = max(0, start - (try await sound.load(.timeRange).start.seconds))
        }
        var clip = AV1Clip(
            frames: frames,
            times: times,
            duration: length.isFinite && length > 0 ? length : times.last! + 1.0 / 24,
            format: format,
            width: Int(dimensions.width),
            height: Int(dimensions.height),
            sound: voiced ? try? AVAudioFile(forReading: url) : nil
        )
        clip.soundStart = soundStart.isFinite ? soundStart : 0
        return clip
    }

    /// Whether `file` is an MP4 whose video track is AV1, from its sample
    /// description, without reading the frames.
    static func isAV1(_ file: URL) -> Bool {
        guard let handle = try? FileHandle(forReadingFrom: file) else { return false }
        defer { try? handle.close() }
        // moov > trak > mdia > minf > stbl > stsd > the first sample entry.
        let path = ["moov", "trak", "mdia", "minf", "stbl", "stsd"]
        func scan(from start: UInt64, to end: UInt64, depth: Int) -> Bool {
            var at = start
            while at + 8 <= end {
                try? handle.seek(toOffset: at)
                guard let header = try? handle.read(upToCount: 16), header.count >= 8 else { return false }
                var size = UInt64(header.prefix(4).reduce(0) { $0 << 8 | UInt32($1) })
                let type = String(decoding: header[4..<8], as: UTF8.self)
                var body = at + 8
                if size == 1, header.count >= 16 {
                    size = header[8..<16].reduce(0) { $0 << 8 | UInt64($1) }
                    body = at + 16
                } else if size == 0 {
                    size = end - at
                }
                guard size >= 8 else { return false }
                if type == path[depth] {
                    if type == "stsd" {
                        // Version and flags, entry count, then the entry's size and type.
                        try? handle.seek(toOffset: body + 8)
                        guard let entry = try? handle.read(upToCount: 8), entry.count == 8 else { return false }
                        if String(decoding: entry[4..<8], as: UTF8.self) == "av01" { return true }
                    } else if scan(from: body, to: at + size, depth: depth + 1) {
                        return true
                    }
                }
                at += size
            }
            return false
        }
        let end = (try? handle.seekToEnd()) ?? 0
        return scan(from: 0, to: end, depth: 0)
    }
}

enum AV1Error: Error {
    case notAV1
    case unreadable
    case decoder
}

// MARK: - Decoding

/// One decoded frame: its picture planes as Metal textures (Y, U and V from
/// dav1d; Y and CbCr from the device's decoder), and what keeps them alive.
struct DecodedFrame: @unchecked Sendable {
    let planes: [MTLTexture]
    let keep: AnyObject?
}

/// Turns compressed AV1 frames into pictures, one after the other.
protocol AV1Backend: AnyObject {
    func decode(_ frame: Data, format: CMFormatDescription) throws -> DecodedFrame?
    /// Forgets the frames before: the next one decoded is frame 0 again.
    func flush()
}

/// Decodes a clip's frames in order on its own queue, a few ahead of the
/// one on screen.
final class AV1Decoder: @unchecked Sendable {
    private let clip: AV1Clip
    private let backend: AV1Backend
    private let queue = DispatchQueue(label: "ai.masko.player.av1", qos: .userInitiated)
    private let lock = NSLock()
    private var frames: [Int: DecodedFrame] = [:]
    private var next = 0
    private var wanted = -1
    private var generation = 0
    /// The generation the backend was last flushed for; only the queue uses it.
    private var flushed = 0
    private var hasFailed = false

    /// The device's AV1 decoder when it has one (iPhone 15 Pro, M3 Macs and
    /// later, which run iOS 17 and macOS 14 at least), dav1d otherwise.
    init?(clip: AV1Clip, device: MTLDevice) {
        self.clip = clip
        if #available(iOS 17.0, macOS 14.0, macCatalyst 17.0, visionOS 1.0, *),
            VTIsHardwareDecodeSupported(AV1Clip.codec),
            let hardware = HardwareAV1(format: clip.format, device: device)
        {
            backend = hardware
        } else if let software = SoftwareAV1(device: device) {
            backend = software
        } else {
            return nil
        }
    }

    var failed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return hasFailed
    }

    /// Decodes up to frame `index`, past the ones already decoded.
    func want(upTo index: Int) {
        lock.lock()
        wanted = max(wanted, min(index, clip.frames.count - 1))
        lock.unlock()
        queue.async { self.pump() }
    }

    /// The latest decoded frame after `shown` and up to `index`; the frames
    /// before it are let go.
    func take(upTo index: Int, after shown: Int) -> (Int, DecodedFrame)? {
        lock.lock()
        defer { lock.unlock() }
        guard let latest = frames.keys.filter({ $0 > shown && $0 <= index }).max(), let frame = frames[latest] else {
            return nil
        }
        frames = frames.filter { $0.key > latest }
        return (latest, frame)
    }

    /// Starts again from frame 0.
    func reset() {
        lock.lock()
        generation += 1
        frames = [:]
        next = 0
        wanted = -1
        lock.unlock()
    }

    private func pump() {
        lock.lock()
        let run = generation
        lock.unlock()
        // A request queued before a reset can run after it. Flushing here, on
        // the queue, before the run's first frame, keeps a flush from landing
        // after frames of the new run were already decoded.
        if flushed != run {
            backend.flush()
            flushed = run
        }
        while true {
            lock.lock()
            guard run == generation, !hasFailed, next <= wanted else {
                lock.unlock()
                return
            }
            let index = next
            lock.unlock()
            let frame: DecodedFrame?
            do {
                frame = try backend.decode(clip.frames[index], format: clip.format)
            } catch {
                lock.lock()
                if run == generation { hasFailed = true }
                lock.unlock()
                return
            }
            lock.lock()
            if run == generation {
                if let frame { frames[index] = frame }
                next = index + 1
            }
            lock.unlock()
        }
    }
}

/// dav1d, for devices without an AV1 decoder. Its 8-bit pictures are copied
/// into three single-channel textures.
final class SoftwareAV1: AV1Backend {
    private var context: OpaquePointer?
    private let device: MTLDevice

    init?(device: MTLDevice) {
        self.device = device
        var settings = Dav1dSettings()
        dav1d_default_settings(&settings)
        settings.n_threads = 2
        // One frame in, its picture out: no frame of delay.
        settings.max_frame_delay = 1
        guard dav1d_open(&context, &settings) == 0, context != nil else { return nil }
    }

    deinit {
        dav1d_close(&context)
    }

    func flush() {
        dav1d_flush(context)
    }

    func decode(_ frame: Data, format: CMFormatDescription) throws -> DecodedFrame? {
        var data = Dav1dData()
        guard let buffer = dav1d_data_create(&data, frame.count) else { throw AV1Error.decoder }
        frame.copyBytes(to: buffer, count: frame.count)
        var decoded: DecodedFrame?
        repeat {
            let sent = dav1d_send_data(context, &data)
            if sent < 0 && sent != -EAGAIN {
                dav1d_data_unref(&data)
                throw AV1Error.decoder
            }
            if let picture = try picture() { decoded = picture }
        } while data.sz > 0
        if decoded == nil { decoded = try picture() }
        return decoded
    }

    private func picture() throws -> DecodedFrame? {
        var picture = Dav1dPicture()
        let got = dav1d_get_picture(context, &picture)
        if got == -EAGAIN { return nil }
        guard got == 0 else { throw AV1Error.decoder }
        defer { dav1d_picture_unref(&picture) }
        guard picture.p.bpc == 8, picture.p.layout == DAV1D_PIXEL_LAYOUT_I420 else { throw AV1Error.decoder }
        let width = Int(picture.p.w)
        let height = Int(picture.p.h)
        let sizes = [(width, height, picture.stride.0), ((width + 1) / 2, (height + 1) / 2, picture.stride.1),
                     ((width + 1) / 2, (height + 1) / 2, picture.stride.1)]
        let sources = [picture.data.0, picture.data.1, picture.data.2]
        var planes: [MTLTexture] = []
        for (i, (w, h, stride)) in sizes.enumerated() {
            let description = MTLTextureDescriptor.texture2DDescriptor(
                pixelFormat: .r8Unorm, width: w, height: h, mipmapped: false)
            description.usage = .shaderRead
            guard let texture = device.makeTexture(descriptor: description), let bytes = sources[i] else {
                throw AV1Error.decoder
            }
            texture.replace(region: MTLRegionMake2D(0, 0, w, h), mipmapLevel: 0, withBytes: bytes, bytesPerRow: stride)
            planes.append(texture)
        }
        return DecodedFrame(planes: planes, keep: nil)
    }
}

/// The device's own AV1 decoder (VideoToolbox), whose pictures reach Metal
/// without a copy.
@available(iOS 17.0, macOS 14.0, macCatalyst 17.0, visionOS 1.0, *)
final class HardwareAV1: AV1Backend {
    private var session: VTDecompressionSession?
    private var cache: CVMetalTextureCache?

    init?(format: CMFormatDescription, device: MTLDevice) {
        let pixels: [CFString: Any] = [
            kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
            kCVPixelBufferMetalCompatibilityKey: true,
        ]
        let decoder: [CFString: Any] = [kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder: true]
        guard CVMetalTextureCacheCreate(nil, nil, device, nil, &cache) == kCVReturnSuccess,
            VTDecompressionSessionCreate(
                allocator: nil, formatDescription: format, decoderSpecification: decoder as CFDictionary,
                imageBufferAttributes: pixels as CFDictionary, outputCallback: nil,
                decompressionSessionOut: &session) == noErr
        else { return nil }
    }

    deinit {
        if let session { VTDecompressionSessionInvalidate(session) }
    }

    func flush() {
        if let session { VTDecompressionSessionWaitForAsynchronousFrames(session) }
    }

    func decode(_ frame: Data, format: CMFormatDescription) throws -> DecodedFrame? {
        guard let session, let cache else { throw AV1Error.decoder }
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(
            allocator: nil, memoryBlock: nil, blockLength: frame.count, blockAllocator: nil,
            customBlockSource: nil, offsetToData: 0, dataLength: frame.count, flags: 0, blockBufferOut: &block) == noErr,
            let block,
            frame.withUnsafeBytes({ CMBlockBufferReplaceDataBytes(
                with: $0.baseAddress!, blockBuffer: block, offsetIntoDestination: 0, dataLength: frame.count) }) == noErr
        else { throw AV1Error.decoder }
        var sample: CMSampleBuffer?
        var size = frame.count
        guard CMSampleBufferCreateReady(
            allocator: nil, dataBuffer: block, formatDescription: format, sampleCount: 1,
            sampleTimingEntryCount: 0, sampleTimingArray: nil, sampleSizeEntryCount: 1,
            sampleSizeArray: &size, sampleBufferOut: &sample) == noErr, let sample
        else { throw AV1Error.decoder }
        var image: CVImageBuffer?
        var result: OSStatus = noErr
        let status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample, flags: [], infoFlagsOut: nil) {
            status, _, buffer, _, _ in
            result = status
            image = buffer
        }
        guard status == noErr, result == noErr else { throw AV1Error.decoder }
        guard let image else { return nil }
        var textures: [CVMetalTexture] = []
        for (plane, pixelFormat) in [(0, MTLPixelFormat.r8Unorm), (1, MTLPixelFormat.rg8Unorm)] {
            var texture: CVMetalTexture?
            guard CVMetalTextureCacheCreateTextureFromImage(
                nil, cache, image, nil, pixelFormat,
                CVPixelBufferGetWidthOfPlane(image, plane), CVPixelBufferGetHeightOfPlane(image, plane),
                plane, &texture) == kCVReturnSuccess, let texture
            else { throw AV1Error.decoder }
            textures.append(texture)
        }
        let planes = textures.compactMap(CVMetalTextureGetTexture)
        guard planes.count == 2 else { throw AV1Error.decoder }
        return DecodedFrame(planes: planes, keep: [image, textures] as AnyObject)
    }
}

// MARK: - Drawing

/// Draws a decoded stacked frame transparent: colour from the top half,
/// transparency from the luma of the bottom half, colour premultiplied by
/// it, BT.709 limited range as the files are made.
@MainActor
final class StackedRenderer {
    static let shared = StackedRenderer()

    let device: MTLDevice
    private let queue: MTLCommandQueue
    private let planar: MTLRenderPipelineState
    private let biplanar: MTLRenderPipelineState

    private static let source = """
        #include <metal_stdlib>
        using namespace metal;
        struct Out { float4 position [[position]]; float2 uv; };
        vertex Out stacked_vertex(uint id [[vertex_id]]) {
            float2 p = float2((id << 1) & 2, id & 2);
            Out o;
            o.position = float4(p * 2.0 - 1.0, 0.0, 1.0);
            o.uv = float2(p.x, 1.0 - p.y);
            return o;
        }
        constexpr sampler s(filter::linear, address::clamp_to_edge);
        float4 transparent(float y, float2 c, float mask) {
            y = (y - 16.0 / 255.0) * 1.164384;
            c -= 128.0 / 255.0;
            float3 rgb = float3(y + 1.792741 * c.y, y - 0.213249 * c.x - 0.532909 * c.y, y + 2.112402 * c.x);
            float a = saturate((mask - 16.0 / 255.0) * 1.164384);
            return float4(saturate(rgb) * a, a);
        }
        fragment float4 stacked_planar(Out in [[stage_in]], texture2d<float> y [[texture(0)]],
                                       texture2d<float> u [[texture(1)]], texture2d<float> v [[texture(2)]]) {
            float2 top = float2(in.uv.x, in.uv.y * 0.5);
            float2 bottom = float2(in.uv.x, 0.5 + in.uv.y * 0.5);
            return transparent(y.sample(s, top).r, float2(u.sample(s, top).r, v.sample(s, top).r), y.sample(s, bottom).r);
        }
        fragment float4 stacked_biplanar(Out in [[stage_in]], texture2d<float> y [[texture(0)]],
                                         texture2d<float> uv [[texture(1)]]) {
            float2 top = float2(in.uv.x, in.uv.y * 0.5);
            float2 bottom = float2(in.uv.x, 0.5 + in.uv.y * 0.5);
            return transparent(y.sample(s, top).r, uv.sample(s, top).rg, y.sample(s, bottom).r);
        }
        """

    private init?() {
        guard let device = MTLCreateSystemDefaultDevice(), let queue = device.makeCommandQueue(),
            let library = try? device.makeLibrary(source: Self.source, options: nil)
        else { return nil }
        func pipeline(_ fragment: String) -> MTLRenderPipelineState? {
            let d = MTLRenderPipelineDescriptor()
            d.vertexFunction = library.makeFunction(name: "stacked_vertex")
            d.fragmentFunction = library.makeFunction(name: fragment)
            d.colorAttachments[0].pixelFormat = .bgra8Unorm
            return try? device.makeRenderPipelineState(descriptor: d)
        }
        guard let planar = pipeline("stacked_planar"), let biplanar = pipeline("stacked_biplanar") else { return nil }
        self.device = device
        self.queue = queue
        self.planar = planar
        self.biplanar = biplanar
    }

    /// Draws `frame` into the layer's next drawable; `done(true)` once the
    /// GPU has drawn it.
    func draw(_ frame: DecodedFrame, into layer: CAMetalLayer, done: @escaping @MainActor (Bool) -> Void) {
        guard let drawable = layer.nextDrawable() else { return done(false) }
        render(frame, into: drawable.texture, present: drawable, done: done)
    }

    /// Draws `frame` into `texture` (and shows `present` when given).
    func render(
        _ frame: DecodedFrame, into texture: MTLTexture, present: MTLDrawable? = nil,
        done: @escaping @MainActor (Bool) -> Void
    ) {
        guard let commands = queue.makeCommandBuffer() else { return done(false) }
        let pass = MTLRenderPassDescriptor()
        pass.colorAttachments[0].texture = texture
        pass.colorAttachments[0].loadAction = .clear
        pass.colorAttachments[0].clearColor = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 0)
        pass.colorAttachments[0].storeAction = .store
        guard let encoder = commands.makeRenderCommandEncoder(descriptor: pass) else { return done(false) }
        encoder.setRenderPipelineState(frame.planes.count == 3 ? planar : biplanar)
        for (i, plane) in frame.planes.enumerated() {
            encoder.setFragmentTexture(plane, index: i)
        }
        encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: 3)
        encoder.endEncoding()
        if let present { commands.present(present) }
        commands.addCompletedHandler { buffer in
            // The frame's textures stay alive until the GPU is done with them.
            let ok = buffer.status == .completed && frame.planes.count > 0
            DispatchQueue.main.async { MainActor.assumeIsolated { done(ok) } }
        }
        commands.commit()
    }
}

// MARK: - Sound

/// The clip's own sound, played in step with its frames.
@MainActor
final class SlotAudio {
    private let file: AVAudioFile
    private let engine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let pitch = AVAudioUnitTimePitch()
    /// Time to schedule the start on the running engine, so it is exact.
    private let lead = 0.05

    init(file: AVAudioFile) {
        self.file = file
        engine.attach(node)
        engine.attach(pitch)
        engine.connect(node, to: pitch, format: file.processingFormat)
        engine.connect(pitch, to: engine.mainMixerNode, format: file.processingFormat)
    }

    /// Starts the engine when it is off. False when there is no sound device
    /// (a test runner, a phone with audio taken): the clip then plays silent.
    func prepare() -> Bool {
        engine.isRunning || (try? engine.start()) != nil
    }

    /// Seconds from asking for a sound to hearing it: the scheduling lead and
    /// the output's delay.
    var delay: Double { lead + OutputDelay.seconds(of: engine) }

    /// Plays from `seconds` into the sound at this rate, and returns when its
    /// first sample is heard, on the `CACurrentMediaTime` clock; nil when it
    /// cannot play.
    func play(from seconds: Double, rate: Double) -> CFTimeInterval? {
        node.stop()
        let start = AVAudioFramePosition(max(0, seconds) * file.processingFormat.sampleRate)
        guard start < file.length, prepare() else { return nil }
        pitch.rate = Float(rate)
        node.scheduleSegment(file, startingFrame: start, frameCount: AVAudioFrameCount(file.length - start), at: nil)
        // CACurrentMediaTime and host time are the same clock.
        let at = CACurrentMediaTime() + lead
        node.play(at: AVAudioTime(hostTime: AVAudioTime.hostTime(forSeconds: at)))
        return at + OutputDelay.seconds(of: engine)
    }

    func pause() {
        node.pause()
    }

    func stop() {
        node.stop()
        engine.stop()
    }
}

/// How long a sample takes from the engine to the speakers.
enum OutputDelay {
    @MainActor
    static func seconds(of engine: AVAudioEngine) -> Double {
        #if os(macOS)
        // What AVFoundation adds up for video: the device's latency and
        // safety offset, its buffer and the stream's latency. Bluetooth
        // headphones report their own long delay here.
        guard let unit = engine.outputNode.audioUnit else { return engine.outputNode.presentationLatency }
        var device = AudioDeviceID(0)
        var size = UInt32(MemoryLayout<AudioDeviceID>.size)
        guard AudioUnitGetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &device, &size) == noErr,
            device != 0
        else { return engine.outputNode.presentationLatency }
        func frames(_ selector: AudioObjectPropertySelector, of object: AudioObjectID, scope: AudioObjectPropertyScope) -> UInt32 {
            var value = UInt32(0)
            var size = UInt32(MemoryLayout<UInt32>.size)
            var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
            return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr ? value : 0
        }
        var rate = Float64(0)
        size = UInt32(MemoryLayout<Float64>.size)
        var address = AudioObjectPropertyAddress(
            mSelector: kAudioDevicePropertyNominalSampleRate, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &rate) == noErr, rate > 0 else {
            return engine.outputNode.presentationLatency
        }
        let out = kAudioObjectPropertyScopeOutput
        var total = frames(kAudioDevicePropertyLatency, of: device, scope: out)
            + frames(kAudioDevicePropertySafetyOffset, of: device, scope: out)
            + frames(kAudioDevicePropertyBufferFrameSize, of: device, scope: out)
        address = AudioObjectPropertyAddress(mSelector: kAudioDevicePropertyStreams, mScope: out, mElement: kAudioObjectPropertyElementMain)
        if AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr, size >= UInt32(MemoryLayout<AudioStreamID>.size) {
            var streams = [AudioStreamID](repeating: 0, count: Int(size) / MemoryLayout<AudioStreamID>.size)
            if AudioObjectGetPropertyData(device, &address, 0, nil, &size, &streams) == noErr, let stream = streams.first {
                total += frames(kAudioStreamPropertyLatency, of: stream, scope: kAudioObjectPropertyScopeGlobal)
            }
        }
        return Double(total) / rate
        #else
        let session = AVAudioSession.sharedInstance()
        return session.outputLatency + session.ioBufferDuration
        #endif
    }
}
