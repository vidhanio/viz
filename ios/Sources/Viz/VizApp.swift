import SwiftUI

@main
struct VizApp: App {
  @State private var backend = Backend()

  var body: some Scene {
    WindowGroup {
      ContentView()
        .environment(backend)
    }
  }
}
