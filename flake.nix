{
  description = "wit: read, search, and navigate GitHub repositories from the CLI and MCP";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Dev shell toolchain: stable Rust with the wasm32 target the
    # wit-snapshot / docs / URL API checks build for.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      modules = import ./nix/modules.nix { inherit self; };
    in
    {
      overlays.default = final: _prev: {
        wit = final.callPackage ./nix/package.nix { };
      };

      packages = forAllSystems (pkgs: rec {
        wit = pkgs.callPackage ./nix/package.nix { };
        default = wit;
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = lib.getExe self.packages.${pkgs.stdenv.hostPlatform.system}.wit;
          meta.description = "Run the wit CLI";
        };
        wit-mcp = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.wit}/bin/wit-mcp";
          meta.description = "Run the wit MCP server";
        };
      });

      nixosModules.default = modules.nixos;
      homeManagerModules.default = modules.homeManager;

      devShells = forAllSystems (
        pkgs:
        let
          rust-bin = rust-overlay.lib.mkRustBin { } pkgs;
          toolchain = rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "clippy"
              "rustfmt"
            ];
            targets = [ "wasm32-unknown-unknown" ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              toolchain
              # aws-lc-sys build
              pkgs.cmake
              pkgs.perl
              # runtime + script dependencies
              pkgs.git
              pkgs.ripgrep
              pkgs.curl
              pkgs.python3
              # showcase/url-api, services/wit-cache, sdk/typescript, docs try-it
              pkgs.nodejs_22
              # formal/: elan reads formal/lean-toolchain and fetches that Lean
              pkgs.elan
              pkgs.nixfmt
            ];
            RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
          };
        }
      );

      checks = forAllSystems (pkgs: {
        wit = self.packages.${pkgs.stdenv.hostPlatform.system}.wit;
        devShell = self.devShells.${pkgs.stdenv.hostPlatform.system}.default;
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
