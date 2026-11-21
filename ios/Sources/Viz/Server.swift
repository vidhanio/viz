import Foundation
import Observation

#if canImport(FoundationNetworking)
  import FoundationNetworking
#endif

/// Protocol v2 types, mirroring `crates/viz-client/src/protocol.rs` and its generated JSON Schema.
enum VizProtocol {
  static let version = 2

  struct Hello: Decodable {
    let protocol_: Int
    let version: String
    let environment: String

    enum CodingKeys: String, CodingKey {
      case protocol_ = "protocol"
      case version, environment
    }
  }

  struct Environment: Decodable, Identifiable {
    enum Status: String, Decodable {
      case local, reachable, unreachable
    }

    let id: String
    let url: String?
    let status: Status
  }

  struct Environments: Decodable {
    let environments: [Environment]
  }

  struct GitDiff: Decodable {
    struct File: Decodable {
      enum Change: String, Decodable {
        case added, deleted, modified, typeChanged
      }

      let path: String
      let change: Change
    }

    let files: [File]
    let patch: String?
  }

  struct GitSummary: Decodable {
    let branch: String?
    let head: String?
    let dirtyFiles: Int
    let ahead: Int?
    let behind: Int?
    let linkedWorktree: Bool
  }

  /// A directory that threads run in; its path is its id.
  struct Project: Decodable, Identifiable {
    let path: String
    let name: String
    let createdAt: Int64
    let git: GitSummary?

    var id: String { path }
  }

  struct GitBranch: Decodable {
    let reference: String
    let head: String
  }

  struct GitBranches: Decodable {
    let branches: [GitBranch]
  }

  struct GitCommit: Decodable {
    let head: String
  }

  struct GitPush: Decodable {
    let remote: String
    let reference: String
  }

  struct GitWorktree: Decodable {
    let path: String
    let branch: String
  }

  struct WorktreeRequest {
    var baseRef: String?
    var branch: String?

    var params: [String: String] {
      var params: [String: String] = [:]
      if let baseRef { params["baseRef"] = baseRef }
      if let branch { params["branch"] = branch }
      return params
    }
  }

  struct ThreadSummary: Decodable, Identifiable {
    enum Status: String, Decodable {
      case idle, working, failed
    }

    let id: Int
    let title: String
    let harness: String
    let harnessName: String
    let parent: Int?
    let parentEnvironment: String?
    let project: String?
    let status: Status
    let archived: Bool
    let updatedAt: Int64
    let unread: Bool
    let pinned: Bool
    let model: String?
    let thinking: String?
    let git: GitSummary?
    let worktree: GitWorktree?
  }

  struct ToolRecord: Decodable {
    enum Status: String, Decodable {
      case pending, in_progress, completed, failed
    }

    let id: String
    let title: String
    let kind: String?
    let status: Status
    let child: Int?
    let childEnvironment: String?
    let output: String?
  }

  /// A transcript entry; `content` is its `kind` with the typed `data`.
  struct Entry: Decodable, Identifiable {
    struct Origin: Decodable {
      let connector: String
      let author: String?
    }

    struct AttachmentRef: Decodable, Identifiable {
      let id: String
      let name: String
      let mime: String
    }

    struct Prompt: Decodable {
      let text: String
      let attachments: [AttachmentRef]?
      let origin: Origin?
    }

    struct Message: Decodable {
      let text: String
      let key: String?
    }

    struct Plan: Decodable {
      struct Item: Decodable {
        let content: String
        let status: String
      }

      let entries: [Item]
    }

    struct RunEnd: Decodable {
      let run: Int?
      let changedFiles: Int?
      let stopReason: String?
      let error: String?
      let answer: String?
    }

    struct TaskResult: Decodable {
      let task: String
      let text: String
    }

    struct ToolUpdate: Decodable {
      let entry: Int
      let record: ToolRecord
    }

    enum Content {
      case continued
      case toolUpdate(ToolUpdate)
      case prompt(Prompt)
      case text(Message)
      case thought(Message)
      case tool(ToolRecord)
      case plan(Plan)
      case run(RunEnd)
      case taskResult(TaskResult)
      case unknown(String)
    }

    let id: Int
    let content: Content

    enum CodingKeys: String, CodingKey {
      case id, kind, data
    }

    init(id: Int, content: Content) {
      self.id = id
      self.content = content
    }

    init(from decoder: Decoder) throws {
      let container = try decoder.container(keyedBy: CodingKeys.self)
      id = try container.decode(Int.self, forKey: .id)
      let kind = try container.decode(String.self, forKey: .kind)
      switch kind {
      case "prompt": content = .prompt(try container.decode(Prompt.self, forKey: .data))
      case "text": content = .text(try container.decode(Message.self, forKey: .data))
      case "thought": content = .thought(try container.decode(Message.self, forKey: .data))
      case "continued": content = .continued
      case "tool_update":
        content = .toolUpdate(try container.decode(ToolUpdate.self, forKey: .data))
      case "tool": content = .tool(try container.decode(ToolRecord.self, forKey: .data))
      case "plan": content = .plan(try container.decode(Plan.self, forKey: .data))
      case "run": content = .run(try container.decode(RunEnd.self, forKey: .data))
      case "task_result":
        content = .taskResult(try container.decode(TaskResult.self, forKey: .data))
      default: content = .unknown(kind)
      }
    }
  }

  struct Entries: Decodable {
    let entries: [Entry]
    let before: Int?
  }

  /// The running turn's uncommitted text and unfinished tools.
  struct LiveTurn: Decodable {
    struct LiveText: Decodable {
      enum Kind: String, Decodable {
        case text, thought
      }

      let key: String
      let kind: Kind
      let text: String
    }

    let text: LiveText?
    let tools: [ToolRecord]
  }

  struct ThreadState: Decodable {
    let historyRevision: UInt64
    struct Queued: Decodable, Identifiable {
      let run: Int
      let text: String
      let attachmentCount: UInt64

      var id: Int { run }
    }

    struct Usage: Decodable {
      let used: Int
      let size: Int
    }

    let status: ThreadSummary.Status
    let harness: String
    let cwd: String
    let queue: [Queued]
    let live: LiveTurn?
    let usage: Usage?
  }

  struct ThreadView: Decodable {
    let thread: Int
    let entries: [Entry]
    let reset: Bool
    /// On a reset: set when older entries remain, the `before` for `thread.entries`.
    let before: Int?
  }

  struct ThreadStateChanged: Decodable {
    let thread: Int
    let state: ThreadState
  }

  struct SearchHit: Decodable, Identifiable {
    let thread: Int
    let entry: Int
    let snippet: String

    var id: String { "\(thread):\(entry)" }
  }

  struct SearchResults: Decodable {
    let hits: [SearchHit]
  }

  struct Projects: Decodable {
    let projects: [Project]
  }

  struct Threads: Decodable {
    let threads: [ThreadSummary]
  }

  struct Launched: Decodable {
    let thread: Int
  }

  struct Sent: Decodable {
    let run: Int
  }
}

/// Connection to a viz environment over a WebSocket at `/v2/rpc`.
/// `hello` picks the environment the whole connection talks to.
@MainActor
@Observable
final class Server {
  enum State: Equatable {
    case disconnected
    case connecting
    case connected(version: String, environment: String)
    case failed(String)
  }

  private(set) var state: State = .disconnected
  /// Bumped by every `connect`, so views can redo per-connection work such as watching.
  private(set) var connection = 0
  private(set) var environments: [VizProtocol.Environment] = []
  private(set) var projects: [VizProtocol.Project] = []
  private(set) var threads: [VizProtocol.ThreadSummary] = []
  private(set) var listError: String?
  /// The watched thread's transcript, in ascending id order.
  private(set) var entries: [Int: [VizProtocol.Entry]] = [:]
  /// The watched thread's state, from `thread.state`.
  private(set) var threadStates: [Int: VizProtocol.ThreadState] = [:]
  /// The `before` cursor for loading older entries of a thread; absent when none remain.
  private(set) var earlier: [Int: Int] = [:]
  private var watched: Int?
  private var endpoint: (url: URL, token: String)?
  private var refreshing = false
  private var refreshRequested = false
  private var task: URLSessionWebSocketTask?
  private var nextId = 0
  private var pending: [Int: CheckedContinuation<Data, Error>] = [:]

  struct RPCError: LocalizedError {
    let code: Int
    let message: String
    var errorDescription: String? { "server error \(code): \(message)" }
  }

  var environment: String? {
    guard case .connected(_, let environment) = state else { return nil }
    return environment
  }

  /// Connects to the server at `url`; `environment` selects a peer for the whole connection.
  func connect(url: URL, token: String, environment: String? = nil) async {
    task?.cancel(with: .goingAway, reason: nil)
    for continuation in pending.values {
      continuation.resume(throwing: URLError(.cancelled))
    }
    pending.removeAll()
    endpoint = (url, token)
    connection += 1
    environments = []
    projects = []
    threads = []
    listError = nil
    entries = [:]
    threadStates = [:]
    earlier = [:]
    watched = nil
    refreshing = false
    refreshRequested = false
    state = .connecting
    var request = URLRequest(url: url.appending(path: "v2/rpc"))
    if var components = URLComponents(url: request.url!, resolvingAgainstBaseURL: false) {
      components.scheme = components.scheme == "https" ? "wss" : "ws"
      request.url = components.url
    }
    request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    let task = URLSession.shared.webSocketTask(with: request)
    self.task = task
    task.resume()
    Task { await receive(on: task) }
    do {
      var params: [String: Any] = [
        "protocol": VizProtocol.version, "client": ["kind": "app", "name": "ios"],
      ]
      if let environment { params["environment"] = environment }
      let data = try await call("hello", params: params)
      let hello = try JSONDecoder().decode(VizProtocol.Hello.self, from: data)
      guard self.task === task else { return }
      state = .connected(version: hello.version, environment: hello.environment)
      await refreshLists(on: task)
    } catch {
      guard self.task === task else { return }
      state = .failed(error.localizedDescription)
    }
  }

  /// Reconnects to the same server with another environment selected.
  func select(environment: String) async {
    guard let endpoint else { return }
    await connect(url: endpoint.url, token: endpoint.token, environment: environment)
  }

  func refresh() async {
    guard let task else { return }
    await refreshLists(on: task)
  }

  func launch(
    project: String? = nil, harness: String? = nil, text: String? = nil,
    worktree: VizProtocol.WorktreeRequest? = nil
  ) async throws -> Int {
    var params: [String: Any] = ["requestId": UUID().uuidString]
    if let project { params["project"] = project }
    if let harness { params["harness"] = harness }
    if let text { params["text"] = text }
    if let worktree { params["worktree"] = worktree.params }
    let data = try await call("thread.launch", params: params)
    return try JSONDecoder().decode(VizProtocol.Launched.self, from: data).thread
  }

  func update(
    thread: Int,
    title: String? = nil,
    archived: Bool? = nil,
    pinned: Bool? = nil,
    unread: Bool? = nil
  ) async throws {
    var params: [String: Any] = ["thread": thread]
    if let title { params["title"] = title }
    if let archived { params["archived"] = archived }
    if let pinned { params["pinned"] = pinned }
    if let unread { params["unread"] = unread }
    _ = try await call("thread.update", params: params)
  }

  func delete(thread: Int, removeWorktree: Bool = false) async throws {
    _ = try await call(
      "thread.delete", params: ["thread": thread, "removeWorktree": removeWorktree])
  }

  func gitBranches(project: String) async throws -> [VizProtocol.GitBranch] {
    let data = try await call("git.branches", params: ["project": project])
    return try JSONDecoder().decode(VizProtocol.GitBranches.self, from: data).branches
  }

  func createWorktree(
    project: String, worktree: VizProtocol.WorktreeRequest
  ) async throws -> VizProtocol.GitWorktree {
    let data = try await call(
      "git.worktree.create", params: ["project": project, "worktree": worktree.params])
    return try JSONDecoder().decode(VizProtocol.GitWorktree.self, from: data)
  }

  func removeWorktree(path: String) async throws {
    _ = try await call("git.worktree.remove", params: ["path": path])
  }

  func gitCommit(thread: Int, message: String, all: Bool = false) async throws
    -> VizProtocol.GitCommit
  {
    let data = try await call(
      "git.commit", params: ["thread": thread, "message": message, "all": all])
    return try JSONDecoder().decode(VizProtocol.GitCommit.self, from: data)
  }

  func gitPush(thread: Int, remote: String? = nil) async throws -> VizProtocol.GitPush {
    var params: [String: Any] = ["thread": thread]
    if let remote { params["remote"] = remote }
    let data = try await call("git.push", params: params)
    return try JSONDecoder().decode(VizProtocol.GitPush.self, from: data)
  }

  func createGitBranch(
    thread: Int, branch: String, baseRef: String? = nil
  ) async throws -> VizProtocol.GitBranch {
    var params: [String: Any] = ["thread": thread, "branch": branch]
    if let baseRef { params["baseRef"] = baseRef }
    let data = try await call("git.branch.create", params: params)
    return try JSONDecoder().decode(VizProtocol.GitBranch.self, from: data)
  }

  func restore(thread: Int, run: Int, rewind: Bool = false) async throws {
    _ = try await call(
      rewind ? "thread.rewind" : "run.restore", params: ["thread": thread, "run": run])
  }

  func fork(
    thread: Int, run: Int, atStart: Bool = false,
    newWorktree: Bool = false, branch: String? = nil, title: String? = nil
  ) async throws -> Int {
    var params: [String: Any] = [
      "thread": thread, "run": run, "boundary": atStart ? "start" : "end",
    ]
    if newWorktree {
      var worktree: [String: String] = [:]
      if let branch { worktree["branch"] = branch }
      params["worktree"] = worktree
    }
    if let title { params["title"] = title }
    let data = try await call("thread.fork", params: params)
    return try JSONDecoder().decode(VizProtocol.Launched.self, from: data).thread
  }

  func diff(thread: Int, run: Int? = nil, summary: Bool = false) async throws -> VizProtocol.GitDiff
  {
    var params: [String: Any] = ["thread": thread, "summary": summary]
    if let run { params["run"] = run }
    let data = try await call(run == nil ? "thread.diff" : "run.diff", params: params)
    return try JSONDecoder().decode(VizProtocol.GitDiff.self, from: data)
  }

  func search(query: String, limit: Int? = nil) async throws -> [VizProtocol.SearchHit] {
    var params: [String: Any] = ["query": query]
    if let limit { params["limit"] = limit }
    let data = try await call("threads.search", params: params)
    return try JSONDecoder().decode(VizProtocol.SearchResults.self, from: data).hits
  }

  func send(text: String, thread: Int) async throws -> Int {
    let data = try await call(
      "thread.send", params: ["thread": thread, "text": text, "requestId": UUID().uuidString]
    )
    return try JSONDecoder().decode(VizProtocol.Sent.self, from: data).run
  }

  func stop(thread: Int) async throws {
    _ = try await call("thread.stop", params: ["thread": thread])
  }

  func removeQueued(thread: Int, run: Int) async throws {
    _ = try await call("queue.remove", params: ["thread": thread, "run": run])
  }

  /// Watches `thread` for `thread.view` and `thread.state`, replacing any previously watched one.
  func watch(thread: Int) async throws {
    if let previous = watched, previous != thread { await unwatch(thread: previous) }
    watched = thread
    _ = try await call("thread.watch", params: ["thread": thread])
  }

  func unwatch(thread: Int) async {
    guard watched == thread else { return }
    watched = nil
    entries[thread] = nil
    threadStates[thread] = nil
    earlier[thread] = nil
    _ = try? await call("thread.unwatch", params: ["thread": thread])
  }

  /// Prepends the page of entries before the oldest loaded one.
  func loadEarlier(thread: Int) async throws {
    guard let before = earlier[thread] else { return }
    let data = try await call(
      "thread.entries", params: ["thread": thread, "before": before, "limit": 100]
    )
    let page = try JSONDecoder().decode(VizProtocol.Entries.self, from: data)
    guard watched == thread, earlier[thread] == before else { return }
    let first = entries[thread]?.first?.id ?? Int.max
    entries[thread, default: []].insert(contentsOf: page.entries.filter { $0.id < first }, at: 0)
    earlier[thread] = page.before
  }

  private func refreshLists(on task: URLSessionWebSocketTask) async {
    guard self.task === task, case .connected = state else { return }
    refreshRequested = true
    guard !refreshing else { return }
    refreshing = true
    defer {
      if self.task === task { refreshing = false }
    }
    while refreshRequested {
      refreshRequested = false
      do {
        let environmentData = try await call("environments.list", params: [:])
        guard self.task === task else { return }
        let projectData = try await call("projects.list", params: [:])
        guard self.task === task else { return }
        let threadData = try await call("threads.list", params: [:])
        guard self.task === task else { return }
        let decoder = JSONDecoder()
        environments = try decoder.decode(VizProtocol.Environments.self, from: environmentData)
          .environments
        projects = try decoder.decode(VizProtocol.Projects.self, from: projectData).projects
        threads = try decoder.decode(VizProtocol.Threads.self, from: threadData).threads
        listError = nil
      } catch {
        guard self.task === task else { return }
        listError = error.localizedDescription
      }
    }
  }

  private func apply(view: VizProtocol.ThreadView) {
    guard watched == view.thread else { return }
    if view.reset {
      entries[view.thread] = view.entries
      earlier[view.thread] = view.before
    } else {
      let last = entries[view.thread]?.last?.id ?? 0
      let added = view.entries.filter { $0.id > last }
      applyToolUpdates(added, thread: view.thread)
      entries[view.thread, default: []].append(contentsOf: added)
    }
  }

  private func applyToolUpdates(_ updates: [VizProtocol.Entry], thread: Int) {
    for entry in updates {
      if case .toolUpdate(let update) = entry.content,
        let index = entries[thread]?.firstIndex(where: { $0.id == update.entry })
      {
        entries[thread]?[index] = .init(id: update.entry, content: .tool(update.record))
      }
    }
  }

  private func notify(method: String, params: Data) {
    let decoder = JSONDecoder()
    switch method {
    case "threads.changed":
      Task { await refresh() }
    case "thread.view":
      if let view = try? decoder.decode(VizProtocol.ThreadView.self, from: params) {
        apply(view: view)
      }
    case "thread.state":
      if let change = try? decoder.decode(VizProtocol.ThreadStateChanged.self, from: params),
        watched == change.thread
      {
        threadStates[change.thread] = change.state
      }
    default:
      break
    }
  }

  /// Sends a request and returns its `result` as JSON data.
  private func call(_ method: String, params: [String: Any]) async throws -> Data {
    guard let task else { throw URLError(.notConnectedToInternet) }
    nextId += 1
    let id = nextId
    let body = try JSONSerialization.data(
      withJSONObject: ["jsonrpc": "2.0", "id": id, "method": method, "params": params]
    )
    return try await withCheckedThrowingContinuation { continuation in
      pending[id] = continuation
      task.send(.string(String(decoding: body, as: UTF8.self))) { error in
        guard let error else { return }
        Task { @MainActor in self.pending.removeValue(forKey: id)?.resume(throwing: error) }
      }
    }
  }

  private func receive(on task: URLSessionWebSocketTask) async {
    while true {
      let message: URLSessionWebSocketTask.Message
      do {
        message = try await task.receive()
      } catch {
        guard self.task === task else { return }
        self.task = nil
        for continuation in pending.values { continuation.resume(throwing: error) }
        pending.removeAll()
        state = .failed(error.localizedDescription)
        return
      }
      guard self.task === task else { return }
      guard case .string(let text) = message,
        let object = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any]
      else { continue }
      if let method = object["method"] as? String {
        let params = object["params"] ?? [String: Any]()
        if let data = try? JSONSerialization.data(withJSONObject: params) {
          notify(method: method, params: data)
        }
        continue
      }
      guard let id = object["id"] as? Int, let continuation = pending.removeValue(forKey: id)
      else { continue }
      if let error = object["error"] as? [String: Any] {
        continuation.resume(
          throwing: RPCError(
            code: error["code"] as? Int ?? 0,
            message: error["message"] as? String ?? ""
          )
        )
      } else {
        let result = object["result"] ?? NSNull()
        let data =
          (try? JSONSerialization.data(withJSONObject: result, options: .fragmentsAllowed))
          ?? Data()
        continuation.resume(returning: data)
      }
    }
  }
}
