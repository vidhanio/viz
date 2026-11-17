# Rune (anomalyco/rune) — desktop UI framework

_Research snapshot 2026-11-17 (scout)._

**Repo**: https://github.com/anomalyco/rune (MIT, © Anomaly; 7,420★). Not the `rune` scripting language (`rune-rs/rune`).

**Crate**: core crate literally named `rune`, **version `0.0.1`**, **not published on crates.io**. `crates.io/crates/rune` is the scripting language **0.14.2**; `rune-ui` → HTTP 404. Repo has **no tags and no releases** (GitHub API returns `[]`). Use a git rev: latest `dev` commit at time of research `0d00d3f2d59df5b764d8d6b2585164fd43c16f48` (2026-11-17). Toolchain pinned: `rust-toolchain.toml` → **1.99.1**, edition 2024.

Companion crates in workspace: `rune-kit` (components), `rune-motion` (springs), `rune-fx`, `rune-mermaid`, `rune-theme-opencode`, `rune-staging`, `rune-taffy`, etc. (`Cargo.toml:1-55`).

### Minimal app (`examples/hello/src/main.rs`, verbatim core)

```rust
use rune::tok::{self, motion, type_};
use rune::{AppSpec, Cx, El, col, el, text};

#[derive(Default)]
struct State { on: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let label = text(if s.on { "Lights on" } else { "Lights off" })
        .font(type_::TITLE).color(t.text);
    let knob = el().key("knob").size(18.0, 18.0).radius(9.0).fill(t.knob)
        .absolute(if s.on { 18.0 } else { 2.0 }, 2.0);
    let switch = el().key("switch").role("switch").label("Lights").checked(s.on).focusable(true)
        .size(38.0, 22.0).radius(11.0)
        .fill(if s.on { tok::accent(0, true) } else { t.control_off })
        .motion(motion::SNAPPY)
        .on_press(|s: &mut State| s.on = !s.on)
        .child(knob);
    col().w_full().h_full().center().gap(16.0).fill(t.bg).child(label).child(switch)
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("on", |s| s.on = true)],
        ..AppSpec::new("Hello", (320.0, 200.0), State::default, view)
    });
}
```

`examples/hello/Cargo.toml`: `rune = { path = "../../crates/rune" }`. Run: `cargo run -p hello`. Headless: `-- --snap hello.png --scene on`.

### Adding it to an external workspace

Rune vendors/patch dependencies, and Cargo does not inherit patches through dependencies — you must replicate them at the same rev (`crates/rune/README.md:2436-2452`):

```toml
[dependencies]
rune = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }

[patch.crates-io]
winit = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }
parley_engine = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }
wgpu-hal = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }
accesskit_windows = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }
accesskit_macos = { git = "https://github.com/anomalyco/rune", rev = "<rev>" }
```

Build deps behind it: `wgpu ~30.0.1` (Vulkan on Linux; D3D12 Windows; Metal on Apple), `winit 0.30.13`, `taffy 0.15`, `parley 0.12`, `swash 0.2.10`, `skrifa 0.44`, `accesskit 0.25.2`, `fontique 0.12` with `fontconfig-dlopen`, `xkbcommon 0.9`, `wayland-client 0.31`, `x11rb 0.13` (`crates/rune/Cargo.toml`).

### Linux build/runtime deps (documented)

From `crates/rune/README.md:2455-2475`:

- Build-time: `libxkbcommon` + `libwayland-client` dev packages (Debian: `libxkbcommon-dev libwayland-dev`; Fedora: `libxkbcommon-devel wayland-devel`; Arch: `libxkbcommon wayland`).
- Windowed/X11 runtime: `libxkbcommon-x11`; without it every window panics: `Library libxkbcommon-x11.so could not be loaded`.
- GPU without hardware: software Vulkan (`mesa-vulkan-drivers`), e.g. `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`.
- Fontconfig: `fontique` uses `fontconfig-dlopen`, so **no headers needed**, but `libfontconfig.so.1` is needed at runtime for system/monospace families (`crates/rune/README.md:1181-1187`).
- X11 is loaded via `x11rb` `dl-libxcb` (`libxcb`, and the X libs winit dlopens: `libX11`, `libXcursor`, `libXi`, `libXrandr`) — _[INFERENCE: nixpkgs attrs, not spelled out in Rune docs]_.
- `dbus` default feature pulls `zbus`/`tokio` on Linux for theme portal, tray, global shortcuts, FileChooser portal.

Nix devShell package set that satisfies the above (attrs are standard nixpkgs; Rune itself names only the first four):

```nix
buildInputs = [
  libxkbcommon wayland wayland-protocols pkg-config
  vulkan-loader mesa            # Vulkan loader + lavapipe/radv
  fontconfig freetype           # runtime for SystemMono/installed families
  libX11 libxcb libXcursor libXi libXrandr
  dbus                          # runtime, for the dbus feature
];
```

Build does **not** need Vulkan headers (wgpu/ash dlopens the loader) nor fontconfig headers.

### Async / tokio story

- **No async UI runtime.** UI is a winit event loop; handlers are sync and re-run `view` between frames. Rune exposes:
  - `AppSpec::setup` — once, receives a `Waker` for background threads.
  - `poll(state, clock, tx)` — before every frame; return `true` to commit.
  - `Tx::wake_after(Duration)` — timer on the unscaled clock; coalesced, no timer thread.
  - `rune::Latest<T>` — debounced, "latest wins" request off the main thread on up to 4 shared workers; `set_waker`, `request`, `take`.
- **Thread → UI bridge**: `rune::App::new(spec)` returns an `AppHandle<S>` that is `Clone + Send + Sync`; call `handle.update(|s, tx| …)` from any thread (wakes the loop, applies in order on the UI thread), plus `open/close/minimize/quit/activate` (`crates/rune/README.md:563-585`). This is the intended tokio bridge.
- **tokio itself** appears only as an optional Linux dependency for `zbus` ("a tokio runtime on each service's thread") and accesskit; it is not the app runtime (`crates/rune/Cargo.toml`, `dbus` feature). Internal blocking (GPU init) uses `pollster`.
- For native dialogs: either poll the future on your own executor, or use callback form `tx.open_file(options, |s, result, tx| …)` which needs "no executor or waker to wire up" (`README:1596-1615`).
- No public "executor" API was found despite the README phrase "the app's async executor" — treat that as _bring your own_ (`[UNVERIFIED]` that a named executor exists).

### Notable for a meta-harness

Every app has headless tooling (debug always; release with `tooling` feature): `--snap out.png`, `--tree tree.json`, `--script demo.json --video out.mp4`, `--trace run.jsonl`, `--list-scenes`, fixed virtual clock; plus `Runtime` test harness (`Runtime::new`, `frame`, `find`, `pointer_down/up`, `presented`). Good fit for an agent-driven UI that must verify itself.

---
