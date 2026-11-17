{
  description = "viz: an all-in-one personal agent and meta-harness";

  nixConfig = {
    extra-substituters = [ "https://nix-community.cachix.org" ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Swift 6.4 + xtool (iOS builds on Linux). Same input as vidhanix.
    # TODO: drop for `nixpkgs` once https://github.com/NixOS/nixpkgs/pull/548520 is merged.
    nixpkgs-xtool.url = "github:NixOS/nixpkgs/pull/548520/head";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
    systems.url = "github:nix-systems/default-linux";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    # Accepts current bun lockfile versions: nix-community/bun2nix#110.
    bun2nix = {
      url = "github:Mic92/bun2nix/lockfile-v2-v3";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ self, ... }:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [ inputs.treefmt-nix.flakeModule ];

      systems = import inputs.systems;

      flake.homeModules = {
        viz = import ./nix/home-module.nix { inherit self; };
        default = self.homeModules.viz;
      };

      perSystem =
        {
          config,
          inputs',
          lib,
          pkgs,
          self',
          system,
          ...
        }:
        let
          swiftPkgs = inputs'.nixpkgs-xtool.legacyPackages;

          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (
            p:
            p.rust-bin.nightly.latest.default.override {
              extensions = [
                "rust-src"
                "rust-analyzer"
              ];
            }
          );

          # Libraries the Rune desktop app links against at build time.
          desktopBuildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];
          # Libraries Rune/wgpu/winit `dlopen` at runtime.
          desktopRuntimeLibs = [
            pkgs.dbus
            pkgs.fontconfig
            pkgs.libxkbcommon
            pkgs.vulkan-loader
            pkgs.wayland
            pkgs.libx11
            pkgs.libxcursor
            pkgs.libxi
            pkgs.libxrandr
            pkgs.libxcb
          ];

          src = craneLib.cleanCargoSource ./.;
          commonArgs = {
            inherit src;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = desktopBuildInputs ++ [ pkgs.dbus ];
            cargoExtraArgs = "--locked --workspace";
            cargoVendorDir = craneLib.vendorCargoDeps {
              inherit src;
              # Crane lists each git crate's files with `cargo package -l`, which rejects Rune's
              # checkout: its vendored crates ship upstream's `Cargo.toml.orig`, and `spikes/` holds
              # a manifest outside Rune's workspace.
              overrideVendorGitCheckout =
                ps: drv:
                if lib.any (p: lib.hasPrefix "git+https://github.com/anomalyco/rune" (p.source or "")) ps then
                  drv.overrideAttrs {
                    preInstall = ''
                      find . -name Cargo.toml.orig -delete
                      rm -rf spikes
                    '';
                  }
                else
                  drv;
            };
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          crate =
            name: extra:
            craneLib.buildPackage (
              commonArgs
              // {
                inherit cargoArtifacts;
                pname = name;
                cargoExtraArgs = "--locked --package ${name}";
                doCheck = false;
              }
              // extra
            );
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [
              inputs.rust-overlay.overlays.default
              inputs.bun2nix.overlays.default
            ];
          };

          packages = {
            viz-backend = pkgs.bun2nix.mkDerivation {
              packageJson = ./backend/package.json;
              src = ./backend;
              bunDeps = pkgs.bun2nix.fetchBunDeps { bunNix = ./backend/bun.nix; };
              # `--bytecode` compiles to CommonJS, which has no top-level await.
              bunCompileToBytecode = false;
              meta.mainProgram = "viz-backend";
            };
            viz = crate "viz" { };
            viz-discord = crate "viz-discord" { };
            viz-imessage = crate "viz-imessage" { };
            viz-desktop = crate "viz-desktop" {
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];
              postFixup = ''
                wrapProgram $out/bin/viz-desktop \
                  --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath desktopRuntimeLibs}
              '';
            };
            default = pkgs.symlinkJoin {
              name = "viz";
              paths = with self'.packages; [
                viz
                viz-backend
                viz-discord
                viz-imessage
                viz-desktop
              ];
            };
          };

          checks = {
            clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- --deny warnings";
              }
            );
            test = craneLib.cargoTest (commonArgs // { inherit cargoArtifacts; });
            backend = pkgs.bun2nix.mkDerivation {
              pname = "viz-backend-check";
              inherit (self'.packages.viz-backend) version src bunDeps;
              buildPhase = ''
                runHook preBuild
                bun check
                bun test
                runHook postBuild
              '';
              installPhase = "touch $out";
            };
          };

          devShells.default = craneLib.devShell {
            packages = [
              # rust
              pkgs.cargo-edit
              pkgs.cargo-deny
              pkgs.pkg-config
              # backend
              pkgs.bun
              pkgs.bun2nix
              # ios
              swiftPkgs.swift
              swiftPkgs.swiftpm
              swiftPkgs.swift-format
              swiftPkgs.sourcekit-lsp
              swiftPkgs.xtool
              pkgs.libimobiledevice # idevicesyslog, idevicedebug, idevicecrashreport, idevicescreenshot
              # nix
              pkgs.nil
              config.treefmt.build.wrapper
            ]
            ++ desktopBuildInputs;

            LD_LIBRARY_PATH = lib.makeLibraryPath desktopRuntimeLibs;

            shellHook = ''
              repo_root=$(git rev-parse --show-toplevel)
              (cd "$repo_root/backend" && bun install --frozen-lockfile --silent)
              if [ ! -e "''${XDG_CONFIG_HOME:-$HOME/.config}/swiftpm/swift-sdks/darwin.artifactbundle" ] \
                && [ ! -e "$HOME/.swiftpm/swift-sdks/darwin.artifactbundle" ]; then
                echo "viz: no Darwin SDK for iOS builds; run 'xtool setup' (see docs/findings/ios-on-linux.md)" >&2
              fi
            '';
          };

          treefmt = {
            programs = {
              nixfmt.enable = true;
              statix.enable = true;
              deadnix.enable = true;
              taplo.enable = true;
              rustfmt = {
                enable = true;
                package = pkgs.rust-bin.nightly.latest.rustfmt;
              };
              swift-format = {
                enable = true;
                package = swiftPkgs.swift-format;
              };
            };
            settings.formatter.bun-fmt = {
              command = lib.getExe pkgs.bun;
              options = [ "fmt" ];
              includes = [
                "*.js"
                "*.ts"
                "*.json"
                "*.md"
                "*.yaml"
                "*.yml"
              ];
            };
            settings.global.excludes = [
              "*.lock"
              "backend/bun.nix"
              ".envrc"
              "*/.gitignore"
              ".gitignore"
            ];
          };
        };
    };
}
