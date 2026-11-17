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
            state = .connected(version: hello.version, root: hello.root)
        } catch {
            state = .failed(error.localizedDescription)
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
                for continuation in pending.values { continuation.resume(throwing: error) }
                pending.removeAll()
                if self.task === task { state = .failed(error.localizedDescription) }
                return
            }
            guard case let .string(text) = message,
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
                    let data = (try? JSONSerialization.data(withJSONObject: result, options: .fragmentsAllowed)) ?? Data()
                    continuation.resume(returning: data)
                }
            }
            // TODO: `conversation.view` notifications (see TODO.md, "iOS").
        }
    }
}
