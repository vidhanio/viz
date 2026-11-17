# 0001: One protocol, two listeners: Unix socket locally, token-authenticated TCP for remote

Status: accepted (2026-11-17)

## Context

Clients differ in where they run: the desktop app and most connectors run on the same machine as the backend; the iOS app never does; a connector may run elsewhere (e.g. iMessage needs a Mac). The user wants remote access to "just work" over tailscale/LAN, like t3code. Options considered: Unix socket only, HTTP REST, gRPC, a WebSocket protocol.

## Decision

- One protocol for every client: JSON-RPC 2.0 over a WebSocket at `/v1/rpc` (plus `GET /v1/health`). Requests/responses for commands, server-pushed notifications for streams (conversation views, connector replies). No separate REST surface to keep in sync.
- Two listeners serving identical routes:
  - Unix socket at `$XDG_RUNTIME_DIR/viz/backend.sock`, mode 0600. File permissions are the authentication; no token.
  - Optional TCP listener (`[backend].listen = "host:port"`), meant for a tailnet (`100.x.y.z`) or LAN address. Requires `[backend].token`; clients send `Authorization: Bearer <token>` (or `?token=` where headers can't be set).
- TLS is not built in. Over tailscale, WireGuard already encrypts; for HTTPS/MagicDNS names, front the listener with `tailscale serve --https=443 http://127.0.0.1:<port>` and bind `listen` to loopback. Never bind a public interface.
- Remote Rust clients set `[client].url` + `[client].token`; with no `[client].url` they use the local socket.

## Consequences

- Bun serves both listeners from the same `fetch`/`websocket` handlers (`backend/src/server.ts`), so local and remote behave identically; tests cover both.
- WebSocket gives the push channel every UI needs (pi-durable `watch()` frames are small delta ops), and JSON-RPC matches ACP's framing, so tooling and mental model are shared.
- A single shared token is coarse. t3code's pairing codes, per-client tokens, and scopes are the model for later (`TODO.md`).
- The iOS app requires `listen` to be configured; it can't use the socket.
