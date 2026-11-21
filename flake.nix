{
  description = "viz: persistent ACP threads and tasks across environments";

  nixConfig = {
    extra-substituters = [ "https://nix-community.cachix.org" ];
    extra-trusted-public-keys = [
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
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

          desktopBuildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];
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
          desktopRuntimeBins = [
            pkgs.xdg-utils
            pkgs.fontconfig.bin
          ];

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              (craneLib.fileset.commonCargoSources ./.)
              ./crates/viz-desktop/themes
              ./crates/viz-desktop/icons
              ./crates/viz-server/src/workspace/schema.sql
            ];
          };
          commonArgs = {
            inherit src;
            strictDeps = true;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = desktopBuildInputs ++ [ pkgs.dbus ];
            cargoExtraArgs = "--locked --workspace";
            cargoVendorDir = craneLib.vendorCargoDeps {
              inherit src;
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
                meta.mainProgram = name;
                cargoExtraArgs = "--locked --package ${name}";
                doCheck = false;
              }
              // extra
            );
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };

          packages = {
            server = crate "viz-server" { };
            viz = crate "viz" { };
            discord = crate "viz-discord" { };
            imessage = crate "viz-imessage" { };
            desktop = crate "viz-desktop" {
              nativeBuildInputs = commonArgs.nativeBuildInputs ++ [ pkgs.makeWrapper ];
              postFixup = ''
                wrapProgram $out/bin/viz-desktop \
                  --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath desktopRuntimeLibs} \
                  --suffix PATH : ${lib.makeBinPath desktopRuntimeBins}
              '';
            };
            default = pkgs.symlinkJoin {
              name = "viz";
              meta.mainProgram = "viz";
              paths = with self'.packages; [
                viz
                server
                discord
                imessage
                desktop
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
          };

          devShells.default = craneLib.devShell {
            packages = [
              pkgs.cargo-edit
              pkgs.cargo-deny
              pkgs.pkg-config
              swiftPkgs.swift
              swiftPkgs.swiftpm
              swiftPkgs.swift-format
              swiftPkgs.sourcekit-lsp
              swiftPkgs.xtool
              pkgs.libimobiledevice
              pkgs.nil
              config.treefmt.build.wrapper
            ]
            ++ desktopBuildInputs;

            LD_LIBRARY_PATH = lib.makeLibraryPath desktopRuntimeLibs;

            shellHook = ''
              if [ ! -e "''${XDG_CONFIG_HOME:-$HOME/.config}/swiftpm/swift-sdks/darwin.artifactbundle" ] \
                && [ ! -e "$HOME/.swiftpm/swift-sdks/darwin.artifactbundle" ]; then
                echo "viz: no Darwin SDK for iOS builds; run 'xtool setup'" >&2
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
            settings.global.excludes = [
              "*.lock"
              ".envrc"
              "*/.gitignore"
              ".gitignore"
            ];
          };
        };
    };
}
