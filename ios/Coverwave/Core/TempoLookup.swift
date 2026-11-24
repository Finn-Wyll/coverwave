import AVFoundation
import Foundation

/// A track's tempo, for the beat clock. Same method as the terminal app
/// (`crates/cw-sources/src/tempo.rs`): find the track on Deezer's public API
/// (no key) by ISRC, else by artist and title; use its `bpm` if set (it
/// usually is not), else download the 30-second preview and estimate the
/// tempo from it in the Rust core.
///
/// Sends only the track's ISRC, or its artist and title, to Deezer.
enum TempoLookup {
    private struct Track: Decodable {
        let id: Int?
        let bpm: Float?
        let preview: String?
    }

    private struct Search: Decodable {
        let data: [Track]
    }

    static func bpm(for track: NowPlaying) async -> Float? {
        guard let found = await find(track) else { return nil }
        if let bpm = found.bpm, (40...220).contains(bpm) { return bpm }
        guard let preview = found.preview.flatMap(URL.init(string:)),
            let (data, response) = try? await URLSession.shared.data(from: preview),
            (response as? HTTPURLResponse)?.statusCode == 200
        else { return nil }
        return await Task.detached(priority: .utility) { estimate(mp3: data) }.value
    }

    private static func find(_ track: NowPlaying) async -> Track? {
        if let isrc = track.isrc,
            let t: Track = await get(path: "track/isrc:\(isrc)"), t.id != nil
        {
            return t
        }
        let artist = track.artist.split(separator: ",").first.map(String.init) ?? track.artist
        let clean = { (s: String) in s.replacingOccurrences(of: "\"", with: "") }
        let query = "artist:\"\(clean(artist).trimmingCharacters(in: .whitespaces))\" track:\"\(clean(track.title))\""
        guard !artist.isEmpty,
            let search: Search = await get(path: "search", query: [
                URLQueryItem(name: "limit", value: "1"), URLQueryItem(name: "q", value: query),
            ]),
            let id = search.data.first?.id
        else { return nil }
        return await get(path: "track/\(id)")
    }

    private static func get<T: Decodable>(path: String, query: [URLQueryItem] = []) async -> T? {
        guard var components = URLComponents(string: "https://api.deezer.com/\(path)") else {
            return nil
        }
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url,
            let (data, response) = try? await URLSession.shared.data(from: url),
            (response as? HTTPURLResponse)?.statusCode == 200
        else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    /// Decodes the preview with AVFoundation, mixes it to mono and asks the
    /// core for a tempo. Nil below the core's confidence threshold.
    private static func estimate(mp3: Data) -> Float? {
        let file = FileManager.default.temporaryDirectory
            .appending(path: "coverwave-preview-\(UUID().uuidString).mp3")
        defer { try? FileManager.default.removeItem(at: file) }
        guard (try? mp3.write(to: file)) != nil,
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
        guard let tempo = estimateTempo(samples: mono, sampleRate: UInt32(buffer.format.sampleRate)),
            tempo.confidence >= minTempoConfidence()
        else { return nil }
        return tempo.bpm
    }
}
