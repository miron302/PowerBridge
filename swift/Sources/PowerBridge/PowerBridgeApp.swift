import SwiftUI
import AppKit

@main
struct PowerBridgeApp: App {
    @StateObject private var model = RunnerModel()

    var body: some Scene {
        WindowGroup("PowerBridge") {
            ContentView()
                .environmentObject(model)
                .frame(minWidth: 860, minHeight: 540)
                .onAppear {
                    NSApplication.shared.setActivationPolicy(.regular)
                    NSApplication.shared.activate(ignoringOtherApps: true)
                }
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Open…") { model.choose() }.keyboardShortcut("o")
            }
        }
    }
}
