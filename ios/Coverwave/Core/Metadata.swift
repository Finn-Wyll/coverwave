import Foundation
import StoreKit

/// Small keyless public APIs that fill in what the music sources leave out:
/// Deezer (for the rhythm preview and a genre) and Apple's lookup (a genre
/// for Apple Music songs). Each sends only an ISRC, a store ID, or the
/// artist and title.
enum WebJSON {
    static func get<T: Decodable>(_ base: String, query: [URLQueryItem] = []) async -> T? {
        guard var components = URLComponents(string: base) else { return nil }
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url,
            let (data, response) = try? await URLSession.shared.data(from: url),
            (response as? HTTPURLResponse)?.statusCode == 200
        else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }
}

/// A song on Deezer, found once per track (by ISRC, else artist and title)
/// and shared by the rhythm map and the genre lookup.
@MainActor
enum Deezer {
    struct Track: Decodable, Sendable {
        struct AlbumRef: Decodable, Sendable { let id: Int? }
        let id: Int?
        let preview: String?
        let duration: Int?
        let album: AlbumRef?
    }

    private struct Search: Decodable {
        let data: [Track]
    }

    private struct Album: Decodable {
        struct Genres: Decodable {
            struct Genre: Decodable { let name: String? }
            let data: [Genre]
        }
        let genres: Genres?
    }

    private static var tracks: [String: Task<Track?, Never>] = [:]

    static func track(for np: NowPlaying) async -> Track? {
        if let task = tracks[np.trackID] { return await task.value }
        let task = Task { await find(np) }
        tracks[np.trackID] = task
        return await task.value
    }

    /// The genres of the song's album on Deezer ("Rock, Alternative").
    static func genre(for np: NowPlaying) async -> String? {
        guard let id = await track(for: np)?.album?.id,
            let album: Album = await WebJSON.get("https://api.deezer.com/album/\(id)")
        else { return nil }
        let names = album.genres?.data.compactMap(\.name).filter { !$0.isEmpty } ?? []
        return names.isEmpty ? nil : names.joined(separator: ", ")
    }

    private static func mainArtist(_ np: NowPlaying) -> String {
        (np.artist.split(separator: ",").first.map(String.init) ?? np.artist)
            .trimmingCharacters(in: .whitespaces)
    }

    private static func find(_ np: NowPlaying) async -> Track? {
        if let isrc = np.isrc,
            let t: Track = await WebJSON.get("https://api.deezer.com/track/isrc:\(isrc)"), t.id != nil
        {
            return t
        }
        let artist = mainArtist(np)
        let clean = { (s: String) in s.replacingOccurrences(of: "\"", with: "") }
        guard !artist.isEmpty,
            let search: Search = await WebJSON.get(
                "https://api.deezer.com/search",
                query: [
                    URLQueryItem(name: "limit", value: "1"),
                    URLQueryItem(name: "q", value: "artist:\"\(clean(artist))\" track:\"\(clean(np.title))\""),
                ]),
            let id = search.data.first?.id
        else { return nil }
        return await WebJSON.get("https://api.deezer.com/track/\(id)")
    }
}

/// The user's Apple Music country, so store IDs resolve in their region.
@MainActor
enum Storefront {
    private static var cached: String?

    static func country() async -> String {
        if let cached { return cached }
        let code = await withCheckedContinuation { (c: CheckedContinuation<String?, Never>) in
            SKCloudServiceController().requestStorefrontCountryCode { code, _ in c.resume(returning: code) }
        }
        let found = code.map { String($0.prefix(2)).lowercased() }
            ?? Locale.current.region?.identifier.lowercased() ?? "us"
        cached = found
        return found
    }
}

/// A genre for songs whose source gave none: Apple's lookup for Apple Music
/// catalogue songs, else the album's genres on Deezer.
@MainActor
enum GenreLookup {
    private struct ITunes: Decodable {
        struct Result: Decodable { let primaryGenreName: String? }
        let results: [Result]
    }

    private static var found: [String: Task<String?, Never>] = [:]

    static func genre(for np: NowPlaying) async -> String? {
        if let task = found[np.trackID] { return await task.value }
        let task = Task { await look(np) }
        found[np.trackID] = task
        return await task.value
    }

    private static func look(_ np: NowPlaying) async -> String? {
        // Apple Music catalogue songs: "am:<store ID>" (all digits).
        if np.trackID.hasPrefix("am:") {
            let id = String(np.trackID.dropFirst(3))
            if !id.isEmpty, id.allSatisfy(\.isNumber), id.count < 15,
                let r: ITunes = await WebJSON.get(
                    "https://itunes.apple.com/lookup",
                    query: [URLQueryItem(name: "id", value: id), URLQueryItem(name: "country", value: await Storefront.country())]),
                let g = r.results.first?.primaryGenreName, !g.isEmpty, g != "Music"
            {
                return g
            }
        }
        return await Deezer.genre(for: np)
    }
}
