import SwiftUI

/// A project-grouped sidebar and the selected conversation's chat.
struct ContentView: View {
  private enum Selection: Hashable {
    case conversation(Int)
    case hit(conversation: Int, entry: Int)

    var conversationID: Int {
      switch self {
      case .conversation(let id), .hit(let id, _): id
      }
    }
  }

  private struct SearchResults {
    let query: String
    let result: Result<[VizProtocol.SearchHit], Error>
  }

  @Environment(Backend.self) private var backend
  // HTTPS: App Transport Security blocks cleartext to non-local hosts. Front the backend with
  // `tailscale serve` (docs/decisions/0001-remote-transport.md).
  @AppStorage("backendURL") private var backendURL = "https://viz.tailnet.ts.net"
  @AppStorage("backendToken") private var backendToken = ""
  @State private var selection: Selection?
  @State private var drafts: [Int: String] = [:]
  @State private var sent: [Int: [String]] = [:]
  @State private var errors: [Int: String] = [:]
  @State private var searchText = ""
  @State private var searchResults: SearchResults?
  @State private var updateError: String?

  var body: some View {
    NavigationSplitView {
      List(selection: $selection) {
        if searchQuery.isEmpty {
          conversationSections
          backendSection
        } else {
          searchSection
        }
CUT_PLACEHOLDER
      }
      .navigationTitle("viz")
      .searchable(text: $searchText, prompt: "Search conversations")
      .task(id: activeSearchQuery) { [query = activeSearchQuery] in
        await loadSearch(query)
      }
      .onChange(of: searchQuery) { _, query in
        if query.isEmpty, case .hit(let id, _) = selection {
          selection = .conversation(id)
        }
      }
      .alert(
        "Update failed",
        isPresented: Binding(
          get: { updateError != nil },
          set: { if !$0 { updateError = nil } }
        )
      ) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(updateError ?? "")
      }
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

  @ViewBuilder
  private var conversationSections: some View {
    let conversations = visibleConversations.sorted { $0.updatedAt > $1.updatedAt }
    let pinned = conversations.filter(\.pinned)
    if !pinned.isEmpty {
      Section("Pinned") {
        ForEach(pinned) { conversation in
          conversationRow(conversation)
        }
      }
    }
    if let orchestrator = conversations.first(where: { $0.kind == .orchestrator && !$0.pinned }) {
      conversationRow(orchestrator)
    }
    let chats = conversations.filter { $0.kind != .orchestrator && !$0.pinned }
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
  }

  private var backendSection: some View {
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

  private var searchQuery: String {
    searchText.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  private var activeSearchQuery: String? {
    guard case .connected = backend.state, !searchQuery.isEmpty else { return nil }
    return searchQuery
  }

  @ViewBuilder
  private var searchSection: some View {
    if let query = activeSearchQuery {
      if let searchResults, searchResults.query == query {
        switch searchResults.result {
        case .success(let hits):
          let conversations = Dictionary(uniqueKeysWithValues: visibleConversations.map { ($0.id, $0) })
          let visibleHits = hits.filter { conversations[$0.conversation] != nil }
          if visibleHits.isEmpty {
            Text("No matches")
              .foregroundStyle(.secondary)
          }
          ForEach(visibleHits) { hit in
            if let conversation = conversations[hit.conversation] {
              NavigationLink(value: Selection.hit(conversation: hit.conversation, entry: hit.entry)) {
                VStack(alignment: .leading, spacing: 4) {
                  conversationTitle(conversation)
                    .lineLimit(1)
                  Text(highlightedSnippet(hit.snippet))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(3)
                }
              }
            }
          }
        case .failure(let error):
          Text(error.localizedDescription)
            .foregroundStyle(.secondary)
        }
      } else {
        ProgressView()
      }
    } else {
      Text("Offline")
        .foregroundStyle(.secondary)
    }
  }

  private func loadSearch(_ query: String?) async {
    guard !Task.isCancelled else { return }
    searchResults = nil
    guard let query else { return }
    do {
      try await Task.sleep(for: .milliseconds(250))
      let hits = try await backend.search(query: query)
      guard !Task.isCancelled else { return }
      searchResults = SearchResults(query: query, result: .success(hits))
    } catch {
      guard !Task.isCancelled else { return }
      searchResults = SearchResults(query: query, result: .failure(error))
    }
  }

  private func highlightedSnippet(_ snippet: String) -> AttributedString {
    var result = AttributedString()
    var start = snippet.unicodeScalars.startIndex
    var emphasized = false
    func append(until end: String.Index) {
      var part = AttributedString(String(snippet.unicodeScalars[start..<end]))
      if emphasized { part.inlinePresentationIntent = .stronglyEmphasized }
      result.append(part)
    }
    for index in snippet.unicodeScalars.indices {
      let scalar = snippet.unicodeScalars[index]
      if scalar == "\u{1}" || scalar == "\u{2}" {
        append(until: index)
        start = snippet.unicodeScalars.index(after: index)
        emphasized = scalar == "\u{1}"
      }
    }
    append(until: snippet.unicodeScalars.endIndex)
    return result
  }

  private var openConversation: VizProtocol.ConversationSummary? {
    if let selection {
      return visibleConversations.first { $0.id == selection.conversationID }
    }
    return visibleConversations.first { $0.kind == .orchestrator }
  }

  private func title(_ conversation: VizProtocol.ConversationSummary) -> String {
    if !conversation.title.isEmpty { return conversation.title }
    return conversation.kind == .orchestrator ? "Orchestrator" : "Untitled"
  }

  private func conversationRow(_ conversation: VizProtocol.ConversationSummary) -> some View {
    NavigationLink(value: Selection.conversation(conversation.id)) {
      HStack {
        conversationTitle(conversation)
          .lineLimit(1)
        Spacer()
        Text(statusLabel(conversation.status))
          .font(.caption)
          .foregroundStyle(.secondary)
      }
    }
    .swipeActions(allowsFullSwipe: false) {
      if conversation.kind != .orchestrator {
        Button(role: .destructive) {
          update(conversation, archived: true)
        } label: {
          Label("Archive", systemImage: "archivebox")
        }
      }
      Button {
        update(conversation, pinned: !conversation.pinned)
      } label: {
        Label(conversation.pinned ? "Unpin" : "Pin", systemImage: conversation.pinned ? "pin.slash" : "pin")
      }
      .tint(.gray)
    }
  }

  private func conversationTitle(_ conversation: VizProtocol.ConversationSummary) -> some View {
    HStack(spacing: 6) {
      if conversation.unread {
        Circle()
          .fill(Color.accentColor)
          .frame(width: 6, height: 6)
          .accessibilityLabel("Unread")
      }
      Text(title(conversation))
        .fontWeight(conversation.unread ? .semibold : .regular)
    }
  }

  private func update(
    _ conversation: VizProtocol.ConversationSummary,
    archived: Bool? = nil,
    pinned: Bool? = nil
  ) {
    Task {
      do {
        try await backend.update(conversation: conversation.id, archived: archived, pinned: pinned)
        if archived == true, selection?.conversationID == conversation.id {
          selection = nil
        }
      } catch {
        updateError = error.localizedDescription
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
    .task(id: conversation.unread ? id : nil) {
      guard conversation.unread else { return }
      do {
        try await backend.update(conversation: id, unread: false)
      } catch {
        if !Task.isCancelled { errors[id] = error.localizedDescription }
      }
    }
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
