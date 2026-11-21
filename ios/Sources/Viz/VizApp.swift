import SwiftUI

@main
struct VizApp: App {
  @State private var server = Server()

  var body: some Scene {
    WindowGroup {
      ContentView()
        .environment(server)
    }
  }
}
