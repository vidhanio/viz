# Swift + xtool on Linux

_Snapshot 2026-11-17._

- Toolchain: Swift 6.4.0 from nixpkgs PR [#548520](https://github.com/NixOS/nixpkgs/pull/548520) ("xtool: init at 1.24.0", approved, waiting on merge; head locked at `619cca8`). The flake input `nixpkgs-xtool` is the same input vidhanix uses (`vidhanix/flake.nix`, `modules/programs/xtool/default.nix`). The dev shell takes `swift`, `swiftpm`, `swift-format`, `sourcekit-lsp`, `xtool` from it.
- The PR adds a NixOS module `programs.xtool` (installs xtool, enables `usbmuxd` for device access; optional `sdk.enable` links a Nix-built Darwin SDK into `~/.swiftpm/swift-sdks`, which needs Xcode added to the store manually). vidhanix enables it on `pc` hosts and persists `~/.config/swiftpm` and `~/.config/xtool`.
- Also adds `xtoolBuildHook` (build iOS apps from SwiftPM in Nix). Not used yet: it needs the Darwin SDK, i.e. an Xcode .xip in the store.
- `xtool new <Name> --skip-setup` template (used for `ios/`): `Package.swift` with exactly one library product (the app), platforms iOS 17 + macOS 14; `xtool.yml` (`version: 1`, `bundleID`); `.bsp/xtool.json` (build server for sourcekit-lsp: `xtool dev build-server`); `.gitignore` ignoring `/.build` and `/xtool`.
- On this machine `~/.swiftpm/swift-sdks` doesn't exist yet: `xtool setup` (Apple ID login + SDK from Xcode .xip) is still to do, so the app has not been built for iOS. `ios/Sources/Viz/Backend.swift` typechecks on Linux (`swiftc -typecheck`, with `FoundationNetworking`); SwiftUI files only compile against the iOS SDK.
- Workflow once set up: `cd ios && xtool dev` (build, sign, install, run on a USB-paired device).
