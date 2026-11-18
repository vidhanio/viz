# 0003: Only the orchestrator runs viz's native agent

The orchestrator owns viz's memory and is the only conversation that uses pi-durable's built-in model and tool loop. App chats and subagents delegate execution to configured ACP runners. They still use the same SQLite store and conversation view protocol; connectors and desktop do not need a second transport.

An app chat binds its runner once. Submissions enter a durable FIFO and one task advances it, so idempotent admission and reply routing work the same way as for the orchestrator. An ACP prompt has no idempotency key: commit its dispatch marker before sending it, and never replay an uncertain turn. A backend restart can reattach at the saved session ID when the runner supports resume or load; a runner without either capability cannot continue an existing session after its process exits.

Old native non-root work is stopped before scheduling resumes. An existing app chat retains its display history, but starts a fresh ACP session on its next submission. Viz does not replay the old turn history into another runner: that would rerun tools or falsely imply the runner had inherited their state.
