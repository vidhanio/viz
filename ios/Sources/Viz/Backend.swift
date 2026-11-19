import Foundation
import Observation

#if canImport(FoundationNetworking)
  // Lets `swiftc -typecheck` run on Linux; iOS gets these types from Foundation.
  import FoundationNetworking
#endif

/// Protocol v1 types, mirroring `backend/src/protocol.ts` (JSON Schema: `protocol/schema.json`).
enum VizProtocol {
  static let version = 1

  struct Hello: Decodable {
    let protocol_: Int
    let version: String
    let root: Int

    enum CodingKeys: String, CodingKey {
      case protocol_ = "protocol"
      case version, root
    }
  }

  struct Submitted: Decodable {
    let submission: Int
  }

  struct Project: Decodable, Identifiable {
    let id: String
    let name: String
    let path: String
    let branch: String?
    let createdAt: Int64
  }

  struct ConversationSummary: Decodable, Identifiable {
    enum Kind: String, Decodable {
      case orchestrator, chat, subagent
    }

    enum Status: String, Decodable {
      case idle, working, failed
    }

    let id: Int
    let title: String
    let kind: Kind
    let parent: Int?
    let project: String?
    let harness: String?
    let status: Status
    let archived: Bool
    let updatedAt: Int64
    let unread: Bool
    let pinned: Bool
    let worktree: String?
  }

  enum OptionValue {
    case string(String)
    case bool(Bool)

    var json: Any {
      switch self {
      case .string(let value): value
      case .bool(let value): value
      }
    }
  }

  struct Worktree {
    let branch: String
    let base: String?
  }

  struct SearchHit: Decodable, Identifiable {
    let conversation: Int
    let entry: Int
    let snippet: String

    var id: Int { entry }
  }

  struct SearchResults: Decodable {
    let hits: [SearchHit]
  }

  struct Projects: Decodable {
    let projects: [Project]
  }

  struct Conversations: Decodable {
    let conversations: [ConversationSummary]
  }

  struct Created: Decodable {
    let conversation: Int
  }
}

/// Connection to a viz backend over its TCP listener (tailnet/LAN), with a bearer token.
///
/// JSON-RPC 2.0 over a WebSocket at `/v1/rpc`. iOS can't reach the Unix socket, so the backend
/// must have `[backend].listen` set.
@MainActor
@Observable
final class Backend {
  enum State: Equatable {
    case disconnected
    case connecting
    case connected(version: String, root: Int)
    case failed(String)
  }

  private(set) var state: State = .disconnected
  private(set) var projects: [VizProtocol.Project] = []
  private(set) var conversations: [VizProtocol.ConversationSummary] = []
  private(set) var listError: String?
  private var refreshing = false
  private var refreshRequested = false
  private var task: URLSessionWebSocketTask?
  private var nextId = 0
  private var pending: [Int: CheckedContinuation<Data, Error>] = [:]

  struct RPCError: LocalizedError {
    let code: Int
    let message: String
    var errorDescription: String? { "backend error \(code): \(message)" }
  }

  func connect(url: URL, token: String) async {
    task?.cancel(with: .goingAway, reason: nil)
    for continuation in pending.values {
      continuation.resume(throwing: URLError(.cancelled))
    }
    pending.removeAll()
    projects = []
    conversations = []
    listError = nil
    refreshing = false
    refreshRequested = false
    state = .connecting
    var request = URLRequest(url: url.appending(path: "v1/rpc"))
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
      let data = try await call(
        "hello",
        params: ["protocol": VizProtocol.version, "client": ["kind": "app", "name": "ios"]]
      )
      let hello = try JSONDecoder().decode(VizProtocol.Hello.self, from: data)
      guard self.task === task else { return }
      state = .connected(version: hello.version, root: hello.root)
      await refreshLists(on: task)
    } catch {
      guard self.task === task else { return }
      state = .failed(error.localizedDescription)
    }
  }

  func create(
    project: String? = nil,
    harness: String? = nil,
    options: [String: VizProtocol.OptionValue]? = nil,
    worktree: VizProtocol.Worktree? = nil
  ) async throws -> Int {
    var params: [String: Any] = [:]
    if let project { params["project"] = project }
    if let harness { params["harness"] = harness }
    if let options { params["options"] = options.mapValues(\.json) }
    if let worktree {
      var value = ["branch": worktree.branch]
      if let base = worktree.base { value["base"] = base }
      params["worktree"] = value
    }
    let data = try await call("conversation.create", params: params)
    return try JSONDecoder().decode(VizProtocol.Created.self, from: data).conversation
  }

  func update(
    conversation: Int,
    title: String? = nil,
    archived: Bool? = nil,
    pinned: Bool? = nil,
    unread: Bool? = nil
  ) async throws {
    var params: [String: Any] = ["conversation": conversation]
    if let title { params["title"] = title }
    if let archived { params["archived"] = archived }
    if let pinned { params["pinned"] = pinned }
    if let unread { params["unread"] = unread }
    _ = try await call("conversation.update", params: params)
  }

  func search(query: String, limit: Int? = nil) async throws -> [VizProtocol.SearchHit] {
    var params: [String: Any] = ["query": query]
    if let limit { params["limit"] = limit }
    let data = try await call("conversations.search", params: params)
    return try JSONDecoder().decode(VizProtocol.SearchResults.self, from: data).hits
  }

  func abort(conversation: Int) async throws {
    _ = try await call("conversation.abort", params: ["conversation": conversation])
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
        let projectData = try await call("projects.list", params: [:])
        guard self.task === task else { return }
        let conversationData = try await call("conversations.list", params: [:])
        guard self.task === task else { return }
        let decoder = JSONDecoder()
        let projects = try decoder.decode(VizProtocol.Projects.self, from: projectData).projects
        let conversations = try decoder.decode(
          VizProtocol.Conversations.self, from: conversationData
        ).conversations
        self.projects = projects
        self.conversations = conversations
        listError = nil
      } catch {
        guard self.task === task else { return }
        listError = error.localizedDescription
      }
    }
  }

  func submit(text: String, conversation: Int? = nil) async throws -> Int {
    var params: [String: Any] = ["text": text]
    if let conversation { params["conversation"] = conversation }
    let data = try await call("conversation.submit", params: params)
    return try JSONDecoder().decode(VizProtocol.Submitted.self, from: data).submission
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
      if let id = object["id"] as? Int, let continuation = pending.removeValue(forKey: id) {
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
      if object["method"] as? String == "conversations.changed" {
        Task { await refreshLists(on: task) }
      }
    }
  }
}
