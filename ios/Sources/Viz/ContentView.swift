import SwiftUI

/// Sidebar of conversations (the orchestrator first) and the selected conversation's chat.
struct ContentView: View {
    @Environment(Backend.self) private var backend
    @AppStorage("backendURL") private var backendURL = "http://viz.tailnet.ts.net:7420"
    @AppStorage("backendToken") private var backendToken = ""
    @State private var draft = ""
    @State private var sent: [String] = []

    var body: some View {
        NavigationSplitView {
            List {
                Section("Conversations") {
                    Label("Orchestrator", systemImage: "sparkles")
                }
                Section("Backend") {
                    TextField("URL", text: $backendURL)
                        .autocorrectionDisabled()
                    SecureField("Token", text: $backendToken)
                    Button("Connect") {
                        guard let url = URL(string: backendURL) else { return }
                        Task { await backend.connect(url: url, token: backendToken) }
                    }
                    Text(status)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .navigationTitle("viz")
        } detail: {
            VStack {
                List(sent, id: \.self) { Text($0) }
                HStack {
                    TextField("Message", text: $draft)
                        .textFieldStyle(.roundedBorder)
                    Button("Send") {
                        let text = draft
                        draft = ""
                        Task {
                            _ = try? await backend.submit(text: text)
                            sent.append(text)
                        }
                    }
                    .disabled(draft.isEmpty)
                }
                .padding()
            }
            .navigationTitle("Orchestrator")
        }
    }

    private var status: String {
        switch backend.state {
        case .disconnected: "Disconnected"
        case .connecting: "Connecting…"
        case let .connected(version, _): "Connected to viz-backend \(version)"
        case let .failed(message): "Failed: \(message)"
        }
    }
}
