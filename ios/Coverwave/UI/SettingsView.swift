import SwiftUI

/// Common choices first; what the cover is made of, one section down.
struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var confirmDelete = false

    var body: some View {
        @Bindable var settings = model.settings
        NavigationStack {
            Form {
                Section {
                    Picker("Scheme", selection: $settings.gestures) {
                        ForEach(GestureScheme.allCases) { Text($0.label).tag($0) }
                    }
                    .pickerStyle(.segmented)
                    GuideTable(scheme: settings.gestures)
                        .padding(.vertical, 4)
                    Button("Show Guide on the Cover") {
                        settings.seenGestureGuide = false
                        dismiss()
                    }
                } header: {
                    Text("Gestures")
                }

                Section {
                    Toggle("Show Lyrics", isOn: $settings.showLyrics)
                    Picker("Style", selection: $settings.lyricStyle) {
                        ForEach(LyricStyleKind.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Scroll", selection: $settings.lyricScroll) {
                        ForEach(LyricScrollKind.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Lyric Size", selection: $settings.lyricSize) {
                        ForEach(LyricSizeKind.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Colours", selection: $settings.lyricPaint) {
                        Text("Accent").tag(LyricPaintKind.accent)
                        Text("Cover").tag(LyricPaintKind.cover)
                    }
                } header: {
                    Text("Lyrics")
                } footer: {
                    Text("Swipe sideways with two fingers to switch between the cover and its lyrics, which take the cover's place with its motion still running. Lyric Size sets how big the sung line is; Detail makes its letters finer without changing their size. Cover colours fill each letter with the cover beneath it, like a window. Lyrics come from LRCLIB (lrclib.net), which is sent the song's title, artist and album.")
                }

                Section {
                    Picker("Motion", selection: $settings.motion) {
                        ForEach(AppSettings.Motion.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Art", selection: $settings.artMode) {
                        ForEach(ArtMode.allCases) { Text($0.label).tag($0) }
                    }
                    Picker("Detail", selection: $settings.artDetail) {
                        ForEach(ArtDetail.allCases) { Text($0.label).tag($0) }
                    }
                } footer: {
                    Text("Finer detail draws the cover with smaller, more numerous characters. Automatic motion pauses when Reduce Motion is on.")
                }

                Section {
                    Toggle("Measure Each Song's Rhythm", isOn: $settings.tempoLookup)
                } header: {
                    Text("Rhythm")
                } footer: {
                    Text(
                        "iOS keeps other apps' audio private, so coverwave measures each song's rhythm from its 30-second Deezer preview with a beat-tracking model on this iPhone, then plays that groove in time with the song. Deezer's previews start 48 seconds in, which is how the beat lines up with the song. Finding the preview sends the song's title and artist to Deezer."
                    )
                }

                if let cover = model.cover, let np = model.nowPlaying {
                    Section("This Cover") {
                        LabeledContent("Song", value: np.title)
                        LabeledContent("Genre", value: AppModel.familyName(cover.genreFamily()).capitalizedFirst)
                        LabeledContent("Motion", value: cover.layerKinds().map(Self.layerName).joined(separator: ", "))
                        LabeledContent("Rhythm", value: model.tempoText)
                    }
                }

                Section {
                    modelRow
                } header: {
                    Text("Cover Analysis")
                } footer: {
                    Text(
                        "Reads each cover's scene (rain, neon, space…) to choose its motion. Uses Apple's MobileCLIP model (\(CoverCore.modelSizeText), research and personal use only), downloaded once from Hugging Face."
                    )
                }

                offlineSection

                Section {
                    ForEach(SourceKind.allCases) { kind in
                        Button {
                            model.choose(kind)
                        } label: {
                            sourceRow(kind)
                        }
                        .disabled(kind != .appleMusic && !model.spotify.isConfigured)
                    }
                    if model.spotify.isConfigured, model.spotify.status != .disconnected {
                        Button("Log Out of Spotify", role: .destructive) { model.spotify.logOut() }
                    }
                } header: {
                    Text("Sources")
                } footer: {
                    Text("Tap a source to follow it.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.page)
        }
        .fontDesign(.monospaced)
        .tint(model.accent)
    }

    @ViewBuilder private var offlineSection: some View {
        @Bindable var settings = model.settings
        let saver = model.saver
        Section {
            Picker("Save Ahead", selection: $settings.saveAhead) {
                ForEach(OfflineSaver.Scope.allCases) { Text($0.label).tag($0) }
            }
            .onChange(of: settings.saveAhead) { _, scope in
                if scope == .off { saver.stop() } else { saver.kick(force: true) }
            }
            Toggle("Use Mobile Data", isOn: $settings.saveOnMobileData)
                .onChange(of: settings.saveOnMobileData) { saver.kick() }
            LabeledContent("Saved") {
                Text(
                    "\(saver.ready) of \(saver.known) songs · "
                        + ByteCountFormatter.string(fromByteCount: saver.diskBytes, countStyle: .file))
            }
            if let activity = saver.activity {
                Text(activity).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
            }
            Button("Save Now") { saver.kick(force: true) }
                .disabled(settings.saveAhead == .off || saver.activity != nil || model.isOfflineMode)
            if model.spotify.needsOfflineLogin {
                Button("Log In Again for Offline") { model.spotify.reauthorize() }
            }
            Button("Delete Saved Data", role: .destructive) { confirmDelete = true }
                .confirmationDialog(
                    "Delete what was saved for offline?", isPresented: $confirmDelete, titleVisibility: .visible
                ) {
                    Button("Delete", role: .destructive) { saver.deleteAll() }
                } message: {
                    Text("Covers, genres, lyrics, rhythm maps and library listings. Spotify's downloads are not touched.")
                }
        } header: {
            Text("Offline")
        } footer: {
            Text(
                "Pick Offline on the start screen (or in Sources) to follow the Spotify app on this iPhone directly while it plays your downloads. coverwave then looks nothing up and uses only what it saved here, so it uses next to no data. Spotify must be open, and it needs the internet to accept the connection (Spotify's rule); after that the downloads play as usual. While online, Save Ahead keeps each song's cover, genre, lyrics and rhythm, and your library's listings. The songs themselves stay in Spotify."
            )
        }
        .onAppear { saver.refreshCounts() }
    }

    @ViewBuilder private var modelRow: some View {
        switch model.core.modelState {
        case .ready:
            LabeledContent("Model", value: "Ready")
        case .loading:
            LabeledContent("Model") { ProgressView() }
        case .downloading(let fraction):
            VStack(alignment: .leading, spacing: 6) {
                Text("Downloading…")
                ProgressView(value: fraction)
            }
        case .missing:
            Button("Download Model") { Task { await model.downloadModel() } }
        case .failed(let message):
            VStack(alignment: .leading, spacing: 4) {
                Button("Try Again") { Task { await model.downloadModel() } }
                Text(message).font(.footnote).foregroundStyle(.secondary)
            }
        }
    }

    private func sourceRow(_ kind: SourceKind) -> some View {
        let source = model.source(kind)
        let status: String =
            switch source.status {
            case .connected: model.activeKind == kind ? "Following" : "Connected"
            case .disconnected: "Not connected"
            case let other: other.message ?? ""
            }
        return LabeledContent {
            Text(status).lineLimit(2).multilineTextAlignment(.trailing)
        } label: {
            Text(model.activeKind == kind ? "> \(kind.name)" : "  \(kind.name)")
                .foregroundStyle(Term.ink)
        }
    }

    /// "pulse_rings" -> "Pulse rings".
    static func layerName(_ kind: String) -> String {
        kind.replacingOccurrences(of: "_", with: " ").capitalizedFirst
    }
}

extension String {
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
