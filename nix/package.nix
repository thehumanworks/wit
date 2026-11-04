# The `wit` CLI (`wit` + `wit-mcp`) built from this workspace.
#
# Versioning follows `crates/wit/Cargo.toml` and dependencies follow
# `Cargo.lock`, so a version bump or dependency update needs no change here.
{
  lib,
  rustPlatform,
  cmake,
  perl,
  makeBinaryWrapper,
  git,
  versionCheckHook,
  # Baked-in shared cloud pack cache (`WIT_DEFAULT_CACHE_URL`, ADR 0009).
  # `null` keeps the release default (the hosted instance); `""` disables it.
  # `WIT_CACHE_URL` still overrides it at run time.
  defaultCacheUrl ? null,
}:

let
  cargoToml = lib.importTOML ../crates/wit/Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = "wit";
  inherit (cargoToml.package) version;

  # Only what cargo needs, so edits to docs, services, SDKs, or the flake
  # itself do not rebuild the binary.
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../README.md
      ../crates
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  env = lib.optionalAttrs (defaultCacheUrl != null) {
    WIT_DEFAULT_CACHE_URL = defaultCacheUrl;
  };

  cargoBuildFlags = [
    "-p"
    "wit"
    "--bins"
  ];

  # aws-lc-sys (rustls' crypto provider) builds its C sources with cmake.
  nativeBuildInputs = [
    cmake
    perl
    makeBinaryWrapper
  ];
  dontUseCmakeConfigure = true;

  # The workspace tests need network access, a writable git config, and
  # sibling worker binaries; they run in CI with `cargo test --workspace`.
  doCheck = false;

  # The disk backend shells out to `git` (cloud pack verification and the
  # `git clone --bare --depth 1` fallback). Suffix so a user's own git wins.
  postInstall = ''
    for bin in wit wit-mcp; do
      wrapProgram "$out/bin/$bin" --suffix PATH : ${lib.makeBinPath [ git ]}
    done
  '';

  nativeInstallCheckInputs = [ versionCheckHook ];
  doInstallCheck = true;
  versionCheckProgramArg = "--version";

  meta = {
    description = "Read, search, and navigate GitHub repositories from the command line and MCP";
    homepage = "https://github.com/thehumanworks/wit";
    mainProgram = "wit";
    platforms = lib.platforms.unix;
  };
}
