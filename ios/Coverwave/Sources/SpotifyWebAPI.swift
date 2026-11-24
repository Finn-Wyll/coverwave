import Foundation

/// The little of Spotify's Web API that App Remote does not cover: a track's
/// ISRC and cover URL, and its first artist's genres. The same calls the
/// terminal app makes (`crates/cw-sources/src/spotify.rs`), so the cover
/// bytes, and with them the seed and the motion, match.
enum SpotifyWebAPI {
    struct Details: Sendable {
        var genre: String?
        var isrc: String?
        var artworkURL: URL?
    }

    private static let base = URL(string: "https://api.spotify.com/v1/")!

    /// `spotify:track:ID` -> `ID` (nil for episodes and ads).
    static func trackID(fromURI uri: String) -> String? {
        let parts = uri.split(separator: ":")
        guard parts.count == 3, parts[1] == "track" else { return nil }
        return String(parts[2])
    }

    private struct Image: Decodable { let url: URL; let width: Int? }
    private struct Album: Decodable { let images: [Image] }
    private struct ArtistRef: Decodable { let id: String? }
    private struct ExternalIDs: Decodable { let isrc: String? }
    private struct Track: Decodable {
        let album: Album
        let artists: [ArtistRef]
        let external_ids: ExternalIDs?
    }
    private struct Artist: Decodable { let genres: [String]? }

    static func details(trackID: String, token: String) async -> Details? {
        guard let track: Track = await get("tracks/\(trackID)", token: token) else { return nil }
        var genre: String?
        if let artistID = track.artists.first?.id,
            let artist: Artist = await get("artists/\(artistID)", token: token),
            let genres = artist.genres, !genres.isEmpty
        {
            genre = genres.joined(separator: ", ")
        }
        let cover = track.album.images.max { ($0.width ?? 0) < ($1.width ?? 0) }
        return Details(genre: genre, isrc: track.external_ids?.isrc, artworkURL: cover?.url)
    }

    private static func get<T: Decodable>(_ path: String, token: String) async -> T? {
        guard let url = URL(string: path, relativeTo: base) else { return nil }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        guard let (data, response) = try? await URLSession.shared.data(for: request),
            (response as? HTTPURLResponse)?.statusCode == 200
        else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }
}
