# 0005: Per-component configuration

Status: accepted (2026-11-19). Supersedes the shared document and parallel loaders in [0002](0002-config-layering.md).

The Rust client connection table below also supersedes the table named in [0001](0001-remote-transport.md); the two listeners and their authentication are unchanged.

## Context

A shared document gave every client access to backend credentials and required two implementations of backend layering. It also coupled clients running on a different machine to the backend's deployment settings.

## Decision

- Only the backend loads `config.toml`, its lexical `config.d/*.toml` layers, or the single file named by `VIZ_CONFIG`. The backend's Home Manager `mutableSettings` option is unchanged.
- Clients share a config directory, not a document. Each reads one file (`cli.toml`, `desktop.toml`, or `connectors/<name>.toml`). `--config` overrides the component-specific environment variable, which overrides the default location. Missing client files use defaults.
- Clients configure the local socket or a remote URL and its token in `[backend]`; they do not derive settings or credentials from the backend file. Home Manager may generate matching socket settings for each file from a shared declaration.
- Desktop schemes are local preferences in `desktop.toml`, selected separately for the system's light and dark appearances. Home Manager can supply the matching Stylix palette.

## Consequences

Install or edit only the files needed for that machine. Move existing client settings out of `config.toml`; coordinate any non-default socket path in the backend and local clients. See [the configuration reference](../config.md).
