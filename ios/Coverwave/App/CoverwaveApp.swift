import SwiftUI

@main
struct CoverwaveApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            PlayerView()
                .environment(model)
                .preferredColorScheme(.dark)
                .task { await model.start() }
                // Spotify's return from authorizing in its app (offline).
                .onOpenURL { url in _ = model.spotify.handle(url: url) }
        }
    }
}
