import CryptoKit
import Foundation

/// The Rust core's cover engine, plus the CLIP model it needs.
///
/// The model (MobileCLIP-S0's vision tower, 45 MB) is downloaded from
/// Hugging Face on first use, checked against a pinned SHA-256 and kept in
/// Application Support. Its weights are licensed for research and personal
/// use only, which is why it is downloaded rather than bundled. Without it,
/// covers still move, using neutral scenes.
@MainActor
@Observable
final class CoverCore {
    enum ModelState: Equatable {
        case missing
        case downloading(Double)
        case loading
        case ready
        case failed(String)
    }

    // Same pinned file as the terminal app (frontends/cw-tui/src/models.rs).
    private static let modelURL = URL(
        string: "https://huggingface.co/Xenova/mobileclip_s0/resolve/757d59c9c6870a76a4b0306f05f5061bca15c39f/onnx/vision_model.onnx"
    )!
    nonisolated private static let modelSHA256 = "17d3c037b1d488c10c50e09f6009ea5a198caef4e0e8f4ea5617b7cb2d067ac0"
    static let modelSizeText = "45 MB"

    private(set) var modelState: ModelState = .missing
    @ObservationIgnored nonisolated let engine = CoverEngine()

    private static var modelFile: URL {
        URL.applicationSupportDirectory.appending(path: "models/mobileclip_s0_vision.onnx")
    }

    /// Loads the model if it was downloaded before.
    func loadIfPresent() async {
        guard modelState == .missing,
            FileManager.default.fileExists(atPath: Self.modelFile.path(percentEncoded: false))
        else { return }
        await load()
    }

    func download() async {
        switch modelState {
        case .missing, .failed: break
        default: return
        }
        modelState = .downloading(0)
        let progress = DownloadProgress { fraction in
            Task { @MainActor [weak self] in
                if case .downloading = self?.modelState { self?.modelState = .downloading(fraction) }
            }
        }
        do {
            let (temp, response) = try await URLSession.shared.download(
                from: Self.modelURL, delegate: progress)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                throw CoreFailure("the download failed")
            }
            let target = Self.modelFile
            try await Task.detached(priority: .utility) {
                let data = try Data(contentsOf: temp, options: .mappedIfSafe)
                guard Self.sha256(data) == Self.modelSHA256 else {
                    throw CoreFailure("the downloaded model is not the expected file")
                }
                try FileManager.default.createDirectory(
                    at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                try? FileManager.default.removeItem(at: target)
                try FileManager.default.moveItem(at: temp, to: target)
            }.value
            await load()
        } catch {
            modelState = .failed(error.localizedDescription)
        }
    }

    private func load() async {
        modelState = .loading
        let engine = self.engine
        let file = Self.modelFile
        do {
            try await Task.detached(priority: .userInitiated) {
                try engine.loadModel(onnx: Data(contentsOf: file, options: .mappedIfSafe))
            }.value
            modelState = .ready
        } catch {
            try? FileManager.default.removeItem(at: file)
            modelState = .failed(error.localizedDescription)
        }
    }

    /// Analyses a cover off the main thread. The CLIP embedding is cached by
    /// cover hash, so a cover is only run through the model once.
    nonisolated func analyse(image: Data, genre: String?) async throws -> AnalysedCover {
        let engine = self.engine
        return try await Task.detached(priority: .userInitiated) {
            let hash = Self.sha256(image)
            let cached = EmbeddingCache.load(hash)
            let cover = try engine.analyse(image: image, genre: genre, embedding: cached)
            if cached == nil, let embedding = cover.embedding() {
                EmbeddingCache.save(embedding, hash: hash)
            }
            return cover
        }.value
    }

    nonisolated static func sha256(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }
}

struct CoreFailure: LocalizedError {
    let errorDescription: String?
    init(_ message: String) { errorDescription = message }
}

/// Reports a download's progress (0...1).
private final class DownloadProgress: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    private let report: @Sendable (Double) -> Void
    private var observation: NSKeyValueObservation?

    init(_ report: @escaping @Sendable (Double) -> Void) {
        self.report = report
    }

    func urlSession(_ session: URLSession, didCreateTask task: URLSessionTask) {
        let report = self.report
        observation = task.progress.observe(\.fractionCompleted) { progress, _ in
            report(progress.fractionCompleted)
        }
    }
}

/// CLIP embeddings by cover hash (512 floats each), in Caches: they can be
/// rebuilt, at the cost of running the model again.
enum EmbeddingCache {
    private static var folder: URL { URL.cachesDirectory.appending(path: "embeddings") }

    static func load(_ hash: String) -> [Float]? {
        guard let data = try? Data(contentsOf: folder.appending(path: "\(hash).f32")),
            data.count % MemoryLayout<Float>.size == 0, !data.isEmpty
        else { return nil }
        return data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    }

    static func save(_ embedding: [Float], hash: String) {
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let data = embedding.withUnsafeBufferPointer { Data(buffer: $0) }
        try? data.write(to: folder.appending(path: "\(hash).f32"), options: .atomic)
    }
}
