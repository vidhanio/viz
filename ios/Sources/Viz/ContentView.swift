import SwiftUI

/// Sidebar of conversations (the orchestrator first) and the selected conversation's chat.
struct ContentView: View {
  @Environment(Backend.self) private var backend
  // HTTPS: App Transport Security blocks cleartext to non-local hosts. Front the backend with
  // `tailscale serve` (docs/decisions/0001-remote-transport.md).
  @AppStorage("backendURL") private var backendURL = "https://viz.tailnet.ts.net"
  @AppStorage("backendToken") private var backendToken = ""
  @State private var selection: Int?
  @State private var drafts: [Int: String] = [:]
  @State private var sent: [Int: [String]] = [:]
  @State private var errors: [Int: String] = [:]

  var body: some View {
    NavigationSplitView {
      List(selection: $selection) {
        if let orchestrator = visibleConversations.first(where: { $0.kind == .orchestrator }) {
          conversationRow(orchestrator)
        }
        ForEach(backend.projects) { project in
          Section(project.name) {
            ForEach(chats.filter { $0.project == project.id }) { conversation in
              conversationRow(conversation)
            }
          }
        }
        let ungrouped = chats.filter { chat in
          !backend.projects.contains { $0.id == chat.project }
        }
        if !ungrouped.isEmpty {
          Section("Chats") {
            ForEach(ungrouped) { conversation in
              conversationRow(conversation)
            }
          }
        }
        Section("Backend") {
          TextField("URL", text: $backendURL)
            .autocorrectionDisabled()
          SecureField("Token", text: $backendToken)
          Button("Connect") {
            guard let url = URL(string: backendURL) else { return }
            selection = nil
            drafts = [:]
            sent = [:]
            errors = [:]
            Task { await backend.connect(url: url, token: backendToken) }
          }
          Text(status)
            .font(.footnote)
            .foregroundStyle(.secondary)
          if let error = backend.listError {
            Text(error)
              .font(.footnote)
              .foregroundStyle(.secondary)
          }
        }
      }
      .navigationTitle("viz")
    } detail: {
      if let conversation = openConversation {
        conversationDetail(conversation)
      } else {
        Text("No conversation open")
          .foregroundStyle(.secondary)
      }
    }
  }

  private var visibleConversations: [VizProtocol.ConversationSummary] {
    backend.conversations.filter { !$0.archived }
  }

  private var chats: [VizProtocol.ConversationSummary] {
    visibleConversations.filter { $0.kind != .orchestrator }
      .sorted { $0.updatedAt > $1.updatedAt }
  }

  private var openConversation: VizProtocol.ConversationSummary? {
    if let selection {
      return visibleConversations.first { $0.id == selection }
    }
    return visibleConversations.first { $0.kind == .orchestrator }
  }

  private func title(_ conversation: VizProtocol.ConversationSummary) -> String {
    if !conversation.title.isEmpty { return conversation.title }
    return conversation.kind == .orchestrator ? "Orchestrator" : "Untitled"
  }

  private func conversationRow(_ conversation: VizProtocol.ConversationSummary) -> some View {
    NavigationLink(value: conversation.id) {
      HStack {
        Text(title(conversation))
          .lineLimit(1)
        Spacer()
        Text(statusLabel(conversation.status))
          .font(.caption)
          .foregroundStyle(.secondary)
      }
    }
  }

  private func statusLabel(_ status: VizProtocol.ConversationSummary.Status) -> String {
    switch status {
    case .idle: "Idle"
    case .working: "Working"
    case .approval: "Approval needed"
    case .failed: "Failed"
    }
  }

  private func conversationDetail(_ conversation: VizProtocol.ConversationSummary) -> some View {
    let id = conversation.id
    let messages = sent[id, default: []]
    let draft = Binding(get: { drafts[id, default: ""] }, set: { drafts[id] = $0 })
    return VStack {
      List(messages.indices, id: \.self) { index in Text(messages[index]) }
      if let error = errors[id] {
        Text(error)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
      HStack {
        TextField("Message", text: draft)
          .textFieldStyle(.roundedBorder)
        if conversation.status == .working {
          Button("Stop") {
            Task {
              do {
                try await backend.abort(conversation: id)
                errors[id] = nil
              } catch {
                errors[id] = error.localizedDescription
              }
            }
          }
        } else {
          Button("Send") {
            let text = draft.wrappedValue
            drafts[id] = ""
            Task {
              do {
                _ = try await backend.submit(text: text, conversation: id)
                sent[id, default: []].append(text)
                errors[id] = nil
              } catch {
                if drafts[id, default: ""].isEmpty { drafts[id] = text }
                errors[id] = error.localizedDescription
              }
            }
          }
          .disabled(draft.wrappedValue.isEmpty)
        }
      }
      .padding()
    }
    .navigationTitle(title(conversation))
  }

  private var status: String {
    switch backend.state {
    case .disconnected: "Disconnected"
    case .connecting: "Connecting…"
    case .connected(let version, _): "Connected to viz-backend \(version)"
    case .failed(let message): "Failed: \(message)"
    }
  }
}
