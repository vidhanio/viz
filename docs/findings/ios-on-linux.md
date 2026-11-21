# iOS development on Linux (late 2026)

_Research snapshot 2026-11-17 (scout); start point: https://github.com/joshuaswarren/omarchy-apple-dev._

## Summary

- **xtool is the viable path** for viz: SwiftPM → iOS app, sign, install, launch on a USB device. No Apple simulator or SwiftUI preview canvas on Linux.
- **omarchy-apple-dev** is the best-maintained Linux recipe: Arch installer, xtool fork, Xcode-.xip SDK extraction, `device-run.sh` with LLDB/console/crash tools, `ship.sh` for TestFlight. Most of it is reusable on NixOS as _concepts_, not scripts.
- **NixOS already has the pieces**: `programs.xtool` enables `usbmuxd`; the dev shell supplies Swift 6.4/xtool. Auth and SDK are still per-user one-time setup.
- **Critical for viz today**: `ContentView.swift` defaults to `http://viz.tailnet.ts.net:7420`. iOS ATS blocks cleartext WebSocket/HTTP to a public hostname. Use HTTPS (Tailscale Serve) or an `Info.plist` ATS exception via `xtool.yml: infoPath`.
- **Release/TestFlight** should be a macOS CI runner or Xcode Cloud. Uploading Linux-built IPAs is technically possible but conflicts with Apple's Xcode/SDK license.

## Q1. omarchy-apple-dev

- Repo: **https://github.com/joshuaswarren/omarchy-apple-dev** — “Build and deploy iOS SwiftUI apps from Omarchy Linux … no Xcode required.” MIT, ~1,869 stars / 122 forks as of the search snapshot.
- Installer: `install-toolchain.sh` (731 lines). Verified on Arch x86_64/aarch64. Installs:
  - pacman: `usbmuxd zip base-devel git libimobiledevice openssl poppler libheif`; `usbmuxd.service` is udev-triggered (do not enable it manually).
  - AUR `swift-bin` (Swift + clang + lldb), builds xtool from a pinned fork, creates `~/pymobile3-venv` with latest PyPI `pymobiledevice3`.
  - Puts the Swift toolchain's `bin` first on `PATH` so its `clang`/`lldb` win.
  - `xtool setup`-equivalent: Xcode `.xip` → `xtool sdk build` → installs `darwin.artifactbundle` into SwiftPM; caches the built SDK so toolchain swaps don't need the `.xip` again.
  - Modes: default **full** (Xcode SDK) and **`--mode no-xcode`** (stubs/headers from the iPhone or Apple's public iOS update; not for App Store).
  - Flags: `--user-only`, `--repair`, `XCODE_XIP=...`.
- Companion kit (`omarchy-ios-kit`, referenced from the README): `device-run.sh` (pair/install/launch, `--lldb`, `--attach`, `--console`, `--symbolicate`, `--rsd`, `--over-tailnet`), `ship.sh`, `tools/xcodeproj2xtool.py`, `tools/host-manifests.py`.
- Reusable on NixOS:
  - Package set/order, especially `libimobiledevice`, `openssl`, `usbmuxd`, `zip`, `poppler`, `libheif`.
  - `usbmuxd` udev/service model — already handled by NixOS.
  - `.bsp/xtool.json` + SourceKit-LSP editor setup.
  - `device-run.sh` patterns for LLDB/console/symbolication.
  - The **Xcode license caveat**: extracting and using Apple's SDK on Linux is the same legal exposure as omarchy's approach; see Q3.
- Files worth reading: `install-toolchain.sh`, `GETTING-STARTED.md`, `FINDINGS.md` (90+ gotchas), `OVER-TAILNET.md`.

## Q2. xtool setup, paths, NixOS interaction

Official Linux guide: **https://xtool.sh/documentation/xtool/installation-linux/**  
Raw doc: `https://raw.githubusercontent.com/xtool-org/xtool/main/Documentation/xtool.docc/Installation-Linux.md`

Prerequisites: Swift 6.4, `usbmuxd`, `libimobiledevice-utils` (optional but useful), Xcode `.xip` (guide currently names **Xcode 27.1**) from **https://developer.apple.com/download/all/?q=Xcode** (browser + Apple ID).

One-time flow:

```sh
xtool setup          # == xtool auth && xtool sdk
# login mode 0: App Store Connect API key (paid membership; recommended)
# login mode 1: Apple ID password + 2FA (any account; private APIs; Anisette service)
# then: Path to Xcode.xip: ~/Downloads/Xcode_27.1.xip
swift sdk list       # must print: darwin
```

Exact storage (from source):

- Auth token: **`~/.config/xtool/data/XTLAuthToken`**. `DirectoryStorage` always uses `~/.config/xtool`; it does **not** honor `XDG_CONFIG_HOME`. It is plain JSON and may contain the App Store Connect private key PEM or Apple ID token/team ID. Treat as a credential; do not sync/commit.
- Darwin SDK: `$XDG_CONFIG_HOME/swiftpm/swift-sdks/darwin.artifactbundle` if `XDG_CONFIG_HOME` is absolute, else **`~/.swiftpm/swift-sdks/darwin.artifactbundle`**. Source: `Sources/PackLib/DarwinSDK.swift:45-70`.
- Build output: `ios/xtool/Viz.app`; BSP scratch path: `ios/.build/index-build`; both ignored by `ios/.gitignore`.
- `xtool sdk` subcommands: `build`, `install [--slim]`, `update`, `remove`, `status`; `xtool auth status|logout|login`.

NixOS `programs.xtool` (PR #548520 module):

```nix
programs.xtool = {
  enable = true;          # installs xtool + services.usbmuxd.enable = true
  package = ...;
  sdk.enable = false;     # default
  # sdk.package = cfg.package.sdk;
};
```

- `enable` → `environment.systemPackages = [xtool]`, `services.usbmuxd.enable = true`.
- NixOS `usbmuxd` module adds `SUBSYSTEM=="usb", ATTR{idVendor}=="05ac", GROUP="usbmux"` and a systemd service (`usbmuxd -U usbmux -v`).
- `sdk.enable` creates a systemd-user tmpfile symlink:
  `L %h/.swiftpm/swift-sdks/darwin.artifactbundle -> ${cfg.sdk.package}`.
  It requires Xcode in the Nix store manually. **Gotcha [INFERENCE]:** if `XDG_CONFIG_HOME` is set (usual), xtool looks in `~/.config/swiftpm/...`, so this symlink is ignored. Also, `xtool sdk install` removes any existing SDK and replaces the symlink, breaking the declarative link. On this machine: keep `sdk.enable = false`, run `xtool auth` + `xtool sdk install` manually, persist `~/.config/swiftpm` and `~/.config/xtool` (vidhanix already does).
- Do **not** run `xtool setup` again after enabling `sdk.enable`; it will remove/replace the SDK link. `xtool auth` alone is safe for login.

## Q3. Day-to-day loop, limits

Build/install:

```sh
cd ios
xtool dev                    # build + sign + install; waits for a device
# first build can take minutes; subsequent builds use SwiftPM cache
xtool dev build              # app only: ./xtool/Viz.app
xtool dev build -c release --ipa --sign   # signed .ipa
xtool install path/to/Viz.ipa             # sign + install
xtool devices                              # list USB/network devices
xtool launch <bundleID> [args...]          # launch via debugserver
xtool uninstall <bundleID>
```

`xtool dev` = `dev run` installs but **does not launch**; the tutorial says tap the app icon. `xtool launch` is the CLI launch path. Source: `DevCommand.swift`, `LaunchCommand.swift`.

First-run device steps: USB connect → Trust → enable **Developer Mode** (Settings → Privacy & Security → Developer Mode) → rerun `xtool dev` → tap app → **Settings → General → VPN & Device Management → [Apple ID] → Trust**.

Bundle ID caveat: xtool may prefix the installed ID (e.g. `XTL-1234.io.vidhan.viz`), especially on free accounts. `xtool launch io.vidhan.viz` may therefore fail. Discover the actual ID with `pymobiledevice3 apps list`, or launch from the home screen / use `idevicesyslog` without a bundle ID.

Logs:

- `idevicesyslog -u <UDID>` — device syslog (filter with `rg viz`). Available from `pkgs.libimobiledevice` (verified in local store).
- `idevicedebug -u <UDID> run <bundleID>` — launches and streams stdout/stderr. Same package.
- `idevicecrashreport -u <UDID> <dir>` — pull crash logs.
- `idevicescreenshot -u <UDID> out.png` — screenshot.

Debugging:

- xtool has **no `debug` subcommand** in 1.24; its CLI list is `dev`, `devices`, `install`, `launch`, `uninstall`, etc. `xtool launch` uses debugserver only to launch, then detaches.
- omarchy's `device-run.sh --lldb` is the proven third-party LLDB loop. It uses `pymobiledevice3 developer debugserver …` and lldb with:
  `platform select remote-ios --sysroot <SDK>`, `target create …/Viz.app/Viz`, `process connect connect://127.0.0.1:<port>`, `process attach --pid <pid>`.
- Raw alternative: `idevicedebugserverproxy -u <UDID> 1234` + lldb, but the app must be launched under debugserver; no turnkey xtool integration. [UNVERIFIED locally]
- Local Swift 6.4 package ships `lldb`, `lldb-server`, `lldb-dap` (verified in `/nix/store/...-swift-6.4.0/bin`).

Wi-Fi:

- `usbmuxd` has a network mode and xtool searches `.usb`, `.network`, `.all`; initial pairing must be USB.
- omarchy measured: Linux wireless RemotePairing path is blocked on iOS 26; on iOS 27, pair once over USB and copy the pairing record to the host on your tailnet, then Wi-Fi/Tailscale install/launch/screenshot/logs works. Treat Wi-Fi as optional; USB is the reliable path.

Limits:

- **No Apple simulator on Linux.** `xtool dev run --simulator` is inside `#if os(macOS)` and uses `xcrun simctl`; unavailable on Linux.
- **No SwiftUI previews canvas.** xtool ≥1.20 supports SwiftUI macros (`#Preview` compiles), but nothing renders it. Apple previews require macOS/Xcode.
- **No asset catalogs, storyboards, or app intents on Linux** in xtool 2.0 beta; 1.24 is more limited. Pure SwiftUI + SwiftPM resources/`iconPath` works.
- **Free Apple ID**: 10 App IDs / 7 days; 3 devices / 7 days; **3 apps per device**; provisioning profiles expire 7 days after issuance; rebuild/reinstall required. Source: https://developer.apple.com/help/account/basics/about-your-developer-account/
- **Push notifications require paid membership** (Apple lists Push Notifications under Apple Developer Program benefits; free personal teams cannot enable the APNs entitlement). Also needs an APNs key/server relay. Source: https://developer.apple.com/programs/whats-included/ and https://developer.apple.com/help/account/reference/supported-capabilities-ios/
- **TestFlight/App Store** require a paid account. Upload requirement since 2026-04-28: Xcode 26+ / iOS 26 SDK. Source: https://developer.apple.com/news/upcoming-requirements/
- **Legal caveat**: Apple's Xcode/SDK license restricts Apple SDK use to Apple-branded hardware (EA2002). omarchy's `ship.sh` reportedly gets Linux-built IPAs accepted by App Store Connect, but isim's analysis argues no compliant all-Linux upload path exists. Use a macOS runner for release. Source: https://github.com/kolabs-dev/isim/blob/main/docs/DISTRIBUTION.md

## Q4. Editors

The local `ios/.bsp/xtool.json`:

```json
{
  "name": "xtool",
  "version": "1.0",
  "bspVersion": "2.2.0",
  "languages": ["c", "cpp", "objective-c", "objective-cpp", "swift"],
  "argv": ["/usr/bin/env", "xtool", "dev", "build-server"]
}
```

SourceKit-LSP natively searches `<workspace>/.bsp/*.json` before SwiftPM and runs that argv (`ExternalBuildServerAdapter.searchForConfig`). No separate BSP client plugin is needed; the editor only has to run `sourcekit-lsp`.

Neovim (nvim-lspconfig, server is now `sourcekit`):

```lua
vim.lsp.config('sourcekit', { cmd = { 'sourcekit-lsp' } })
vim.lsp.enable('sourcekit')
```

nvim-lspconfig's root detection already includes `.bsp` and `buildServer.json`.

VS Code: install `swiftlang.swift-vscode`; it uses SourceKit-LSP and picks up `.bsp`. Linux is supported. Zed: install the Swift extension; it uses SourceKit-LSP and `lldb-dap` from `PATH`. It will find the dev-shell `sourcekit-lsp`.

## Q5. Recommendations for viz

Dev shell (`flake.nix`):

- Add `pkgs.libimobiledevice` — gives `idevicesyslog`, `idevicedebug`, `idevicedebugserverproxy`, `idevicecrashreport`, `idevicescreenshot`, `idevicepair`, `ideviceinfo` (verified present locally).
- Optionally add `pkgs.pymobiledevice3`; nixpkgs version is old (7.7.0) vs iOS 27 needs, so prefer a user venv/`uv tool` if using LLDB/Tailscale.
- Keep `programs.xtool.sdk.enable = false` until the `%h/.swiftpm` vs `$XDG_CONFIG_HOME` issue is fixed or Xcode is in the store; use manual `xtool auth` + `xtool sdk install`.
- Consider a `shellHook` warning: if `swift sdk list | grep -q darwin` fails, print the `xtool setup` instructions instead of failing later.

AGENTS.md iOS section:

- Replace “needs the Darwin SDK (`xtool setup`)” with exact steps, paths, and the Xcode 27.1 URL.
- Document: `xtool dev` installs but does not launch; use `xtool launch` or the home screen; on-device bundle ID may be `XTL-*.io.vidhan.viz`.
- Document logs: `idevicesyslog`, `idevicedebug run`, crash reports; LLDB is third-party/advanced.
- Document the 7-day/3-app free-account limits and that push/TestFlight need a paid account.
- Note no simulator/previews; device testing is mandatory.
- **ATS**: either change `ContentView` default to `https://…` served by `tailscale serve`, or add `ios/Info.plist` with `NSAppTransportSecurity` exception and `xtool.yml: infoPath: Info.plist`. Prefer HTTPS.
- Note `@AppStorage("serverToken")` is not Keychain (already in TODO); don't put a real token there.

TODO.md:

- [ ] `xtool setup` (free password or paid API key), Xcode 27.1 `.xip`, `swift sdk list` → `darwin`.
- [ ] First USB device build; record bundle ID prefix and trust/Developer Mode steps.
- [ ] Add `libimobiledevice` to dev shell and document logs/debug.
- [ ] Fix ATS: HTTPS/Tailscale Serve or `Info.plist` exception.
- [ ] App icon via `xtool.yml: iconPath` (PNG 1024×1024).
- [ ] Release path: paid account + `macos-26`/`xcode-27` GitHub runner or Xcode Cloud; not Linux-only.
- [ ] Revisit isim for simulator/previews (experimental; own SDK, Swift 6.2.4; not production build).
- [ ] Keep push notifications out of scope until paid account + APNs relay.

## Other notable tools

- **isim** — Linux iOS simulator/toolchain: own SDK (no Apple binaries), SwiftUI partial, `isim build/test`, `isim preview` renders `#Preview` to PNG, `--os 17|18|26|27`, 12 iPhones/4 iPads. Needs Swift 6.2.4 exactly + LLVM 21. Cannot build/sign device apps or upload; not a substitute for xtool’s Apple-SDK build. https://github.com/kolabs-dev/isim
- **Tomacco** — Omarchy-oriented agent-first mobile dev, 296 typed ops, MCP; virtual iOS device needs your own restore image + boot ticket + 16 GiB GPU; NixOS explicitly unsupported due to loader. Heavy; not recommended for viz now. https://tomacco.dev/
- **Darling** — macOS runtime on Linux, not iOS; Xcode itself doesn't run. https://github.com/darlinghq/darling
- **Theos** — jailbreak/tweak build system; supports Linux but targets jailbroken devices, not App Store apps. https://theos.dev/
- **swift-sdk-generator** — Linux/FreeBSD targets only; cannot generate an Apple/Darwin SDK. https://github.com/swiftlang/swift-sdk-generator
- **osxcross** — macOS cross-toolchain, not an iOS app/SwiftUI workflow. https://github.com/tpoechtrager/osxcross
- **xcodes** — macOS Xcode-version manager; irrelevant on Linux. **XcodeGen/Tuist** — generate `.xcodeproj` for macOS/CI; xtool has `xtool dev generate-xcode-project` for the same purpose, and xtool 2.0 beta can read `.xcodeproj`.
- **Remote Mac CI** — GitHub `macos-26` (M1 3-core/7 GB, Xcode 26.6 default), `macos-26-intel`, `xcode-27` label; $0.062/min private, free for public repos. `xcodebuild archive` + `-exportArchive` + `xcrun altool --upload-app` (or Xcode Cloud: 25 h/month with paid membership). Sources: https://docs.github.com/en/actions/reference/runners/github-hosted-runners, https://docs.github.com/en/billing/concepts/product-billing/github-actions, https://developer.apple.com/xcode-cloud/
- **xtool 2.0 beta (2026-11-16)** — Xcode project support, SwiftBuild, hot reload **on macOS simulator only**; Linux still lacks asset catalogs/app intents/storyboards. 1.24.0 remains the stable choice. https://github.com/xtool-org/xtool/releases/tag/2.0.0-beta.1
