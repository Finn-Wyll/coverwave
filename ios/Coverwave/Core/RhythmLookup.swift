import AVFoundation
import CryptoKit
import Foundation

/// A song's rhythm map: its 30-second preview, run through the Beat This!
/// beat tracker in the Rust core, gives the tempo, metre, bar phase and the
/// song's own groove (see `cw_core::rhythm`).
///
/// The preview comes from Deezer's public API (by ISRC, else artist and
/// title; no key). Only the ISRC, or the artist and title, are sent. Apple's
/// previews are not used: their terms allow only promotional use. Maps are
/// cached per track, so a song is measured once.
enum RhythmLookup {
    /// The bundled model, loaded on first use (a second or two).
    private static let engine = Task.detached(priority: .utility) { () -> RhythmEngine? in
        guard let url = Bundle.main.url(forResource: "BeatThis-small", withExtension: "onnx"),
            let data = try? Data(contentsOf: url, options: .mappedIfSafe)
        else { return nil }
        return try? RhythmEngine(onnx: data)
    }

    /// Without `network`, only a saved map.
    static func rhythm(for track: NowPlaying, network: Bool = true) async -> Rhythm? {
        if let cached = RhythmCache.load(track.trackID) { return cached }
        guard network, let deezer = await Deezer.track(for: track),
            let preview = deezer.preview.flatMap(URL.init(string:)),
            let (data, response) = try? await URLSession.shared.data(from: preview),
            (response as? HTTPURLResponse)?.statusCode == 200,
            let engine = await engine.value
        else { return nil }
        let start = clipStart(deezer, playing: track)
        let rhythm = await Task.detached(priority: .utility) { () -> Rhythm? in
            guard let (samples, rate) = decodeMono(data) else { return nil }
            return try? engine.measure(samples: samples, sampleRate: rate, clipStart: start)
        }.value
        if let rhythm { RhythmCache.save(rhythm, for: track.trackID) }
        return rhythm
    }

    // MARK: Finding a preview

    /// Deezer cuts every preview from 48 seconds into the track (measured
    /// on library songs: 47.97 to 48.02s whenever the recordings match), so
    /// the preview's beat grid extends to the whole song.
    static let deezerPreviewStart = 48.0

    /// Where the preview starts in the song being played, if that is known:
    /// the track must be long enough for a preview from 48s, and Deezer's
    /// version must be the same length (within 3s) as the one playing.
    private static func clipStart(_ deezer: Deezer.Track, playing: NowPlaying) -> Double? {
        guard let d = deezer.duration, Double(d) >= deezerPreviewStart + 30 else { return nil }
        if let playing = playing.duration, abs(playing - Double(d)) > 3 { return nil }
        return deezerPreviewStart
    }

    /// Decodes a preview (MP3 or AAC) with AVFoundation and mixes it to mono.
    private static func decodeMono(_ encoded: Data) -> ([Float], UInt32)? {
        let file = FileManager.default.temporaryDirectory
            .appending(path: "coverwave-preview-\(UUID().uuidString).m4a")
        defer { try? FileManager.default.removeItem(at: file) }
        guard (try? encoded.write(to: file)) != nil,
            let audio = try? AVAudioFile(forReading: file),
            audio.length > 0,
            let buffer = AVAudioPCMBuffer(
                pcmFormat: audio.processingFormat, frameCapacity: AVAudioFrameCount(audio.length)),
            (try? audio.read(into: buffer)) != nil,
            let channels = buffer.floatChannelData
        else { return nil }
        let frames = Int(buffer.frameLength)
        let count = Int(buffer.format.channelCount)
        var mono = [Float](repeating: 0, count: frames)
        for c in 0..<count {
            let samples = channels[c]
            for i in 0..<frames { mono[i] += samples[i] / Float(count) }
        }
        return (mono, UInt32(buffer.format.sampleRate))
    }
}

/// Rhythm maps by track ID, as JSON, kept with what is saved for offline.
enum RhythmCache {
    private static var folder: URL { OfflineStore.folder.appending(path: "rhythm") }

    private static func file(_ trackID: String) -> URL {
        let name = SHA256.hash(data: Data(trackID.utf8)).prefix(12).map { String(format: "%02x", $0) }.joined()
        return folder.appending(path: "\(name).json")
    }

    static func load(_ trackID: String) -> Rhythm? {
        guard let json = try? String(contentsOf: file(trackID), encoding: .utf8) else { return nil }
        return try? Rhythm.fromJson(json: json)
    }

    static func save(_ rhythm: Rhythm, for trackID: String) {
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? rhythm.toJson().write(to: file(trackID), atomically: true, encoding: .utf8)
    }
}
