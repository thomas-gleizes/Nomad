import AppKit
import SwiftUI

@main
struct NomadApp: App {
    @State private var model = AppModel()

    init() {
        // App accessoire : présente dans la barre de menus, pas dans le Dock.
        NSApplication.shared.setActivationPolicy(.accessory)
        let model = model
        Task { @MainActor in model.start() }
    }

    var body: some Scene {
        MenuBarExtra("Nomad", systemImage: "cursorarrow.rays") {
            MenuBarView(model: model)
        }

        Window("Nomad — Console", id: "console") {
            ConsoleView(model: model)
        }
        .windowResizability(.contentMinSize)
    }
}
