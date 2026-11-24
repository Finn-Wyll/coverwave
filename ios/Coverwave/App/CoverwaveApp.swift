import SwiftUI

@main
struct CoverwaveApp: App {
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            PlayerView()
                .environment(model)
                .preferredColorScheme(.dark)
                .onOpenURL { model.handleOpenURL($0) }
                .task { await model.start() }
        }
    }
}
