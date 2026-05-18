{
  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";
    nixpkgs.url = "nixpkgs";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs =
    inputs@{
      nixpkgs,
      flake-parts,
      rust-overlay,
      ...
    }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      perSystem =
        {
          system,
          pkgs,
          ...
        }:
        let
          isLinux = pkgs.stdenv.isLinux;
          extensions = [
            "rust-analyzer"
            "rustfmt"
            "clippy"
            "rust-src"
          ];
          crossTargets = [
            "x86_64-unknown-linux-musl"
            "x86_64-pc-windows-gnu"
            "aarch64-apple-darwin"
            "x86_64-apple-darwin"
          ];
          rust-toolchain = pkgs.rust-bin.stable."1.91.0".default.override {
            inherit extensions;
            targets = crossTargets;
          };
          # macOS SDK needed on Linux hosts to cross-compile for Darwin
          # (frameworks like CoreFoundation, CoreServices).
          macOS-SDK = pkgs.fetchurl {
            url = "https://github.com/phracker/MacOSX-SDKs/releases/download/11.3/MacOSX11.3.sdk.tar.xz";
            hash = "sha256-zU8Ip1V3FFuPBSRaKXX3yBQB116VNdz/u4ee4d7vy/Q=";
          };
          macOS-SDK-unpacked = pkgs.runCommand "MacOSX11.3.sdk" { } ''
            mkdir -p $out
            tar -xJf ${macOS-SDK} -C $out --strip-components=1
          '';
        in
        {
          _module.args.pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
            overlays = [ (import rust-overlay) ];
          };
          packages = rec {
            docbake = pkgs.rustPlatform.buildRustPackage {
              pname = "docbake";
              version = "0.1.12";
              src = ./.;
              cargoLock = {
                lockFile = ./Cargo.lock;
              };
              doCheck = false;
              meta = {
                description = "Multi-format Markdown documentation compiler";
                mainProgram = "docbake";
              };
            };
            default = docbake;
          };
          devShells.default = pkgs.mkShell {
            buildInputs = [
            ];
            nativeBuildInputs = [
              rust-toolchain
              pkgs.cargo-zigbuild
              pkgs.zig
              pkgs.dpkg 
              pkgs.debian-devscripts 
            ] ++ pkgs.lib.optionals isLinux [
              pkgs.d2
              pkgs.drawio-headless
              pkgs.plantuml
              pkgs.mermaid-cli
            ];
            name = "docbake";
            shellHook =
              ''
                # cargo-zigbuild generates zig-cc wrapper scripts inside the
                # cache dir and passes their paths to cc-rs via CC_<target>.
                # The path MUST be absolute so that build-scripts running from
                # other working directories can find the wrappers.
                export CARGO_ZIGBUILD_CACHE_DIR="$PWD/target/zigbuild-cache"

                # cargo-zigbuild acts as a dlltool wrapper when invoked via
                # a x86_64-w64-mingw32-dlltool symlink. The windows-sys crate's
                # build script needs this on PATH to create import libraries.
                _zigbuild_bin="$CARGO_ZIGBUILD_CACHE_DIR/bin"
                mkdir -p "$_zigbuild_bin"
                ln -sf "$(which cargo-zigbuild)" "$_zigbuild_bin/x86_64-w64-mingw32-dlltool"
                export PATH="$_zigbuild_bin:$PATH"
              ''
              + pkgs.lib.optionalString isLinux ''
                # macOS SDK for cross-compiling Darwin targets from Linux.
                export SDKROOT="${macOS-SDK-unpacked}"
              '';
          };
        };
    };
}
