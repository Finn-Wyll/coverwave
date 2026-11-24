import AVFoundation

/// Optional: hears the music through the microphone and analyses it in the
/// Rust core, so the cover reacts to the actual song rather than a beat
/// clock. iOS does not let one app hear another's audio, so this is the
/// only way to react to the real sound.
///
/// The session mixes with other apps (Spotify and Music keep playing), and
/// samples go straight to the analyser: nothing is recorded or sent.
@MainActor
final class MicListener {
    private let engine = AVAudioEngine()
    private(set) var analyser: LiveAnalyser?
    /// When the analyser last heard more than silence.
    private var lastSound = Date.distantPast

    var isRunning: Bool { analyser != nil }

    func start() async -> Bool {
        if isRunning { return true }
        guard await AVAudioApplication.requestRecordPermission() else { return false }
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(
                .playAndRecord, mode: .measurement,
                options: [.mixWithOthers, .defaultToSpeaker, .allowBluetoothA2DP])
            try session.setActive(true)
        } catch {
            return false
        }
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 else { return false }
        let analyser = LiveAnalyser(sampleRate: UInt32(format.sampleRate))
        input.installTap(onBus: 0, bufferSize: 1024, format: format, block: Self.tap(analyser))
        do {
            try engine.start()
        } catch {
            input.removeTap(onBus: 0)
            return false
        }
        self.analyser = analyser
        lastSound = .now
        return true
    }

    /// The tap runs on an audio thread, so its block is built outside the
    /// main actor. The analyser locks internally.
    private nonisolated static func tap(_ analyser: LiveAnalyser) -> AVAudioNodeTapBlock {
        { buffer, _ in
            guard let channel = buffer.floatChannelData?[0] else { return }
            let count = Int(buffer.frameLength)
            analyser.push(samples: Array(UnsafeBufferPointer(start: channel, count: count)))
        }
    }

    func stop() {
        guard isRunning else { return }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
        analyser = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Levels if the microphone is hearing something. After a few seconds
    /// of silence (headphones, a quiet room) this returns nil and the beat
    /// clock takes over again.
    func levels(at date: Date) -> AudioLevels? {
        guard let analyser else { return nil }
        let levels = analyser.levels()
        if levels.level > 0.02 { lastSound = date }
        return date.timeIntervalSince(lastSound) < 3 ? levels : nil
    }
}
