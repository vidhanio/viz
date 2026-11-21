import SwiftUI

/// A project-grouped sidebar and the selected thread's chat.
struct ContentView: View {
  private enum Selection: Hashable {
    case thread(Int)
    case hit(thread: Int, entry: Int)

    var threadID: Int {
      switch self {
      case .thread(let id), .hit(let id, _): id
      }
    }

    var entry: Int? {
      guard case .hit(_, let entry) = self else { return nil }
      return entry
    }
  }

  private struct SearchResults {
    let query: String
    let result: Result<[VizProtocol.SearchHit], Error>
  }

  @Environment(Server.self) private var server
  @AppStorage("serverURL") private var serverURL = "https://viz.example.com"
  @AppStorage("serverToken") private var serverToken = ""
  @State private var selection: Selection?
  @State private var drafts: [Int: String] = [:]
  @State private var errors: [Int: String] = [:]
  @State private var searchText = ""
  @State private var searchResults: SearchResults?
  @State private var pendingDelete: VizProtocol.ThreadSummary?
  @State private var notice: (title: String, detail: String)?

  var body: some View {
    NavigationSplitView {
      List(selection: $selection) {
        if searchQuery.isEmpty {
          threadSections
          serverSection
        } else {
          searchSection
        }
      }
      .navigationTitle("viz")
      .searchable(text: $searchText, prompt: "Search threads")
      .toolbar {
        ToolbarItem(placement: .primaryAction) {
          newMenu
        }
      }
      .task(id: activeSearchQuery) { [query = activeSearchQuery] in
        await loadSearch(query)
      }
      .onChange(of: searchQuery) { _, query in
        if query.isEmpty, case .hit(let id, _) = selection {
          selection = .thread(id)
        }
      }
      .alert(
        pendingDelete.map { "Delete “\(title($0))”?" } ?? "Delete thread?",
        isPresented: Binding(
          get: { pendingDelete != nil },
          set: { if !$0 { pendingDelete = nil } }
        ),
        presenting: pendingDelete
      ) { thread in
        Button("Cancel", role: .cancel) {}
        Button("Delete", role: .destructive) { delete(thread) }
      } message: { _ in
        Text("This can't be undone.")
      }
      .alert(
        notice?.title ?? "",
        isPresented: Binding(
          get: { notice != nil },
          set: { if !$0 { notice = nil } }
        )
      ) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(notice?.detail ?? "")
      }
    } detail: {
      if let thread = selectedThread {
        ThreadDetail(
          thread: thread,
          title: title(thread),
          target: selection?.entry,
          draft: Binding(
            get: { drafts[thread.id, default: ""] }, set: { drafts[thread.id] = $0 }),
          failure: Binding(get: { errors[thread.id] }, set: { errors[thread.id] = $0 }),
          open: open(thread:environment:)
        )
        .id(thread.id)
      } else {
        Text("No thread open")
          .foregroundStyle(.secondary)
      }
    }
  }

  private var connected: Bool {
    server.environment != nil
  }

  private var visibleThreads: [VizProtocol.ThreadSummary] {
    server.threads.filter { !$0.archived }
  }

  private var newMenu: some View {
    Menu {
      Section("New thread") {
        Button("No project") { launch(project: nil) }
        ForEach(server.projects) { project in
          Button(project.name) { launch(project: project.path) }
        }
      }
    } label: {
      Label("New", systemImage: "square.and.pencil")
    }
    .disabled(!connected)
  }

  @ViewBuilder
  private var threadSections: some View {
    let threads = visibleThreads.sorted { $0.updatedAt > $1.updatedAt }
    let pinned = threads.filter(\.pinned)
    if !pinned.isEmpty {
      Section("Pinned") {
        ForEach(pinned) { thread in
          threadRow(thread)
        }
      }
    }
    let rest = threads.filter { !$0.pinned }
    ForEach(server.projects) { project in
      Section(project.name) {
        ForEach(rest.filter { $0.project == project.path }) { thread in
          threadRow(thread)
        }
      }
    }
    let ungrouped = rest.filter { thread in
      !server.projects.contains { $0.path == thread.project }
    }
    if !ungrouped.isEmpty {
      Section("Threads") {
        ForEach(ungrouped) { thread in
          threadRow(thread)
        }
      }
    }
  }

  private var serverSection: some View {
    Section("Server") {
      TextField("URL", text: $serverURL)
        .autocorrectionDisabled()
      SecureField("Token", text: $serverToken)
      Button("Connect") {
        guard let url = URL(string: serverURL) else { return }
        resetLocal()
        Task { await server.connect(url: url, token: serverToken) }
      }
      if let current = server.environment, !server.environments.isEmpty {
        Picker(
          "Environment",
          selection: Binding(get: { current }, set: { switchEnvironment($0) })
        ) {
          ForEach(server.environments) { environment in
            Text(environmentLabel(environment)).tag(environment.id)
          }
        }
      }
      Text(status)
        .font(.footnote)
        .foregroundStyle(.secondary)
      if let error = server.listError {
        Text(error)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    }
  }

  private func environmentLabel(_ environment: VizProtocol.Environment) -> String {
    environment.status == .unreachable ? "\(environment.id) (unreachable)" : environment.id
  }

  private var searchQuery: String {
    searchText.trimmingCharacters(in: .whitespacesAndNewlines)
  }

  private var activeSearchQuery: String? {
    guard connected, !searchQuery.isEmpty else { return nil }
    return searchQuery
  }

  @ViewBuilder
  private var searchSection: some View {
    if let query = activeSearchQuery {
      if let searchResults, searchResults.query == query {
        switch searchResults.result {
        case .success(let hits):
          let threads = Dictionary(uniqueKeysWithValues: visibleThreads.map { ($0.id, $0) })
          let visibleHits = hits.filter { threads[$0.thread] != nil }
          if visibleHits.isEmpty {
            Text("No matches")
              .foregroundStyle(.secondary)
          }
          ForEach(visibleHits) { hit in
            if let thread = threads[hit.thread] {
              NavigationLink(value: Selection.hit(thread: hit.thread, entry: hit.entry)) {
                VStack(alignment: .leading, spacing: 4) {
                  threadTitle(thread)
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
      let hits = try await server.search(query: query)
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

  private var selectedThread: VizProtocol.ThreadSummary? {
    guard let selection else { return nil }
    return server.threads.first { $0.id == selection.threadID }
  }

  private func title(_ thread: VizProtocol.ThreadSummary) -> String {
    thread.title.isEmpty ? "Untitled" : thread.title
  }

  private func threadRow(_ thread: VizProtocol.ThreadSummary) -> some View {
    NavigationLink(value: Selection.thread(thread.id)) {
      HStack {
        threadTitle(thread)
          .lineLimit(1)
        Spacer()
        Text(statusLabel(thread.status))
          .font(.caption)
          .foregroundStyle(.secondary)
      }
    }
    .swipeActions(allowsFullSwipe: false) {
      Button(role: .destructive) {
        pendingDelete = thread
      } label: {
        Label("Delete", systemImage: "trash")
      }
      Button {
        update(thread, archived: true)
      } label: {
        Label("Archive", systemImage: "archivebox")
      }
      .tint(.orange)
      Button {
        update(thread, pinned: !thread.pinned)
      } label: {
        Label(
          thread.pinned ? "Unpin" : "Pin",
          systemImage: thread.pinned ? "pin.slash" : "pin")
      }
      .tint(.gray)
    }
  }

  private func threadTitle(_ thread: VizProtocol.ThreadSummary) -> some View {
    HStack(spacing: 6) {
      if thread.unread {
        Circle()
          .fill(Color.accentColor)
          .frame(width: 6, height: 6)
          .accessibilityLabel("Unread")
      }
      Text(title(thread))
        .fontWeight(thread.unread ? .semibold : .regular)
    }
  }

  private func resetLocal() {
    selection = nil
    drafts = [:]
    errors = [:]
  }

  private func switchEnvironment(_ environment: String) {
    guard environment != server.environment else { return }
    resetLocal()
    Task { await server.select(environment: environment) }
  }

  /// Opens a thread, first switching environments when it lives in another one.
  private func open(thread: Int, environment: String?) {
    guard let environment, environment != server.environment else {
      selection = .thread(thread)
      return
    }
    resetLocal()
    Task {
      await server.select(environment: environment)
      selection = .thread(thread)
    }
  }

  private func launch(project: String?) {
    Task {
      do {
        let id = try await server.launch(project: project)
        await server.refresh()
        selection = .thread(id)
      } catch {
        notice = ("New thread failed", error.localizedDescription)
      }
    }
  }

  private func update(
    _ thread: VizProtocol.ThreadSummary,
    archived: Bool? = nil,
    pinned: Bool? = nil
  ) {
    Task {
      do {
        try await server.update(thread: thread.id, archived: archived, pinned: pinned)
        if archived == true, selection?.threadID == thread.id {
          selection = nil
        }
      } catch {
        notice = ("Update failed", error.localizedDescription)
      }
    }
  }

  private func delete(_ thread: VizProtocol.ThreadSummary) {
    let parent = thread.parentEnvironment == nil ? thread.parent : nil
    Task {
      do {
        try await server.delete(thread: thread.id)
        if selection?.threadID == thread.id {
          selection = parent.map { .thread($0) }
        }
        drafts[thread.id] = nil
        errors[thread.id] = nil
      } catch {
        notice = ("Delete failed", error.localizedDescription)
      }
    }
  }

  private func statusLabel(_ status: VizProtocol.ThreadSummary.Status) -> String {
    switch status {
    case .idle: "Idle"
    case .working: "Working"
    case .failed: "Failed"
    }
  }

  private var status: String {
    switch server.state {
    case .disconnected: "Disconnected"
    case .connecting: "Connecting…"
    case .connected(let version, let environment):
      "Connected to viz-server \(version) (\(environment))"
    case .failed(let message): "Failed: \(message)"
    }
  }
}

/// A watched thread's transcript, live turn, queue, and composer.
private struct ThreadDetail: View {
  @Environment(Server.self) private var server
  let thread: VizProtocol.ThreadSummary
  let title: String
  /// A search hit's entry to scroll to.
  let target: Int?
  @Binding var draft: String
  @Binding var failure: String?
  let open: (_ thread: Int, _ environment: String?) -> Void

  init(
    thread: VizProtocol.ThreadSummary,
    title: String,
    target: Int?,
    draft: Binding<String>,
    failure: Binding<String?>,
    open: @escaping (_ thread: Int, _ environment: String?) -> Void
  ) {
    self.thread = thread
    self.title = title
    self.target = target
    _draft = draft
    _failure = failure
    self.open = open
  }

  var body: some View {
    let entries = server.entries[thread.id] ?? []
    let state = server.threadStates[thread.id]
    let status = state?.status ?? thread.status
    VStack(spacing: 0) {
      ScrollViewReader { proxy in
        List {
          if server.earlier[thread.id] != nil {
            Button("Load earlier") {
              Task { await loadEarlier() }
            }
          }
          ForEach(entries) { entry in
            entryRow(entry)
          }
          if let live = state?.live {
            liveRows(live, committed: entries)
          }
          if let queue = state?.queue, !queue.isEmpty {
            Section("Queued") {
              ForEach(queue) { queued in
                Text(queued.text)
                  .foregroundStyle(.secondary)
                  .swipeActions {
                    Button(role: .destructive) {
                      removeQueued(queued.run)
                    } label: {
                      Label("Remove", systemImage: "trash")
                    }
                  }
              }
            }
          }
        }
        .listStyle(.plain)
        .onChange(of: entries.last?.id, initial: true) { _, last in
          if target == nil, let last { proxy.scrollTo(last, anchor: .bottom) }
        }
        .onChange(of: entries.first?.id, initial: true) { _, _ in
          reveal(proxy)
        }
        .onChange(of: target) { _, _ in
          reveal(proxy)
        }
      }
      if let failure {
        Text(failure)
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
      HStack {
        TextField("Message", text: $draft, axis: .vertical)
          .lineLimit(1...5)
          .textFieldStyle(.roundedBorder)
        if status == .working {
          Button("Stop") { stop() }
        }
        Button("Send") { send() }
          .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
      }
      .padding()
      if let usage = state?.usage, usage.size > 0 {
        Text("\(usage.used * 100 / usage.size)% of context used")
          .font(.caption2)
          .foregroundStyle(.secondary)
          .padding(.bottom, 4)
      }
    }
    .navigationTitle(title)
    .toolbar {
      if let parent = thread.parent {
        ToolbarItem(placement: .primaryAction) {
          Button {
            open(parent, thread.parentEnvironment)
          } label: {
            Label("Parent", systemImage: "arrow.up.left")
          }
        }
      }
    }
    .task(id: server.connection) {
      do {
        try await server.watch(thread: thread.id)
      } catch {
        if !Task.isCancelled { failure = error.localizedDescription }
      }
    }
    .onDisappear { [id = thread.id, server = server] in
      Task { await server.unwatch(thread: id) }
    }
    .task(id: thread.unread ? thread.id : nil) {
      guard thread.unread else { return }
      do {
        try await server.update(thread: thread.id, unread: false)
      } catch {
        if !Task.isCancelled { failure = error.localizedDescription }
      }
    }
  }

  @ViewBuilder
  private func entryRow(_ entry: VizProtocol.Entry) -> some View {
    switch entry.content {
    case .prompt(let prompt):
      VStack(alignment: .trailing, spacing: 4) {
        Text(prompt.text)
          .padding(8)
          .background(Color.accentColor.opacity(0.15), in: RoundedRectangle(cornerRadius: 10))
        ForEach(prompt.attachments ?? []) { attachment in
          Label(attachment.name, systemImage: "paperclip")
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        if let origin = prompt.origin {
          Text(origin.author.map { "\($0) via \(origin.connector)" } ?? origin.connector)
            .font(.caption2)
            .foregroundStyle(.secondary)
        }
      }
      .frame(maxWidth: .infinity, alignment: .trailing)
    case .text(let message):
      Text(markdown(message.text))
        .textSelection(.enabled)
    case .thought(let message):
      thoughtRow(message.text)
    case .tool(let tool):
      toolRow(tool)
    case .plan(let plan):
      VStack(alignment: .leading, spacing: 4) {
        ForEach(plan.entries.indices, id: \.self) { index in
          let item = plan.entries[index]
          Label(item.content, systemImage: planIcon(item.status))
        }
      }
      .font(.callout)
    case .run(let run):
      if let error = run.error {
        Label(error, systemImage: "exclamationmark.triangle")
          .font(.footnote)
          .foregroundStyle(.secondary)
      } else if run.stopReason == "cancelled" {
        Text("Stopped")
          .font(.footnote)
          .foregroundStyle(.secondary)
      } else if run.stopReason != "end_turn" {
        Text("Ended: \(run.stopReason)")
          .font(.footnote)
          .foregroundStyle(.secondary)
      }
    case .taskResult(let result):
      VStack(alignment: .leading, spacing: 4) {
        Label("Task result", systemImage: "arrow.turn.down.left")
          .font(.caption)
          .foregroundStyle(.secondary)
        Text(markdown(result.text))
          .textSelection(.enabled)
      }
    case .unknown(let kind):
      Text("Unsupported entry: \(kind)")
        .font(.caption)
        .foregroundStyle(.secondary)
    }
  }

  /// The live turn's text and unfinished tools; text already committed under the same key is skipped.
  @ViewBuilder
  private func liveRows(_ live: VizProtocol.LiveTurn, committed: [VizProtocol.Entry]) -> some View {
    if let text = live.text,
      !committed.reversed().contains(where: { entry in
        switch entry.content {
        case .text(let message), .thought(let message): message.key == text.key
        default: false
        }
      })
    {
      switch text.kind {
      case .text: Text(markdown(text.text))
      case .thought: thoughtRow(text.text)
      }
    }
    ForEach(live.tools.indices, id: \.self) { index in
      toolRow(live.tools[index])
    }
  }

  private func thoughtRow(_ text: String) -> some View {
    DisclosureGroup("Thinking") {
      Text(text)
        .font(.callout)
        .foregroundStyle(.secondary)
    }
    .foregroundStyle(.secondary)
  }

  private func toolRow(_ tool: VizProtocol.ToolRecord) -> some View {
    HStack {
      Image(systemName: toolIcon(tool.status))
        .foregroundStyle(.secondary)
      Text(tool.title)
        .font(.callout)
        .lineLimit(2)
      Spacer()
      if let child = tool.child {
        Button("Open") { open(child, tool.childEnvironment) }
          .buttonStyle(.borderless)
      }
    }
  }

  private func toolIcon(_ status: VizProtocol.ToolRecord.Status) -> String {
    switch status {
    case .pending: "circle"
    case .in_progress: "ellipsis.circle"
    case .completed: "checkmark.circle"
    case .failed: "xmark.circle"
    }
  }

  private func planIcon(_ status: String) -> String {
    switch status {
    case "completed": "checkmark.circle"
    case "in_progress": "ellipsis.circle"
    default: "circle"
    }
  }

  private func markdown(_ text: String) -> AttributedString {
    (try? AttributedString(
      markdown: text,
      options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    )) ?? AttributedString(text)
  }

  /// Scrolls to the target entry, loading older pages until it is loaded.
  private func reveal(_ proxy: ScrollViewProxy) {
    guard let target else { return }
    let entries = server.entries[thread.id] ?? []
    if entries.contains(where: { $0.id == target }) {
      proxy.scrollTo(target, anchor: .center)
    } else if let first = entries.first, target < first.id, server.earlier[thread.id] != nil {
      Task { await loadEarlier() }
    }
  }

  private func loadEarlier() async {
    do {
      try await server.loadEarlier(thread: thread.id)
    } catch {
      failure = error.localizedDescription
    }
  }

  private func send() {
    let text = draft
    draft = ""
    Task {
      do {
        _ = try await server.send(text: text, thread: thread.id)
        failure = nil
      } catch {
        if draft.isEmpty { draft = text }
        failure = error.localizedDescription
      }
    }
  }

  private func stop() {
    Task {
      do {
        try await server.stop(thread: thread.id)
        failure = nil
      } catch {
        failure = error.localizedDescription
      }
    }
  }

  private func removeQueued(_ run: Int) {
    Task {
      do {
        try await server.removeQueued(thread: thread.id, run: run)
      } catch {
        failure = error.localizedDescription
      }
    }
  }
}
