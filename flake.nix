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
          extensions = [
            "rust-analyzer"
            "rustfmt"
            "clippy"
            "rust-src"
          ];
          crossTargets = [
            "x86_64-unknown-linux-musl"
            "x86_64-pc-windows-gnu"
          ];
          rust-toolchain = pkgs.rust-bin.stable."1.91.0".default.override {
            inherit extensions;
            targets = crossTargets;
          };
        in
        {
          _module.args.pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          devShells.default = pkgs.mkShell {
            buildInputs = [
            ];
            nativeBuildInputs = [
              rust-toolchain
              pkgs.cargo-zigbuild
              pkgs.zig
            ];
            name = "docbake";
            CARGO_ZIGBUILD_CACHE_DIR = "target/zigbuild-cache";
            shellHook = ''
              # cargo-zigbuild acts as a dlltool wrapper when invoked via
              # a x86_64-w64-mingw32-dlltool symlink. The windows-sys crate's
              # build script needs this on PATH to create import libraries.
              _zigbuild_bin="target/zigbuild-cache/bin"
              mkdir -p "$_zigbuild_bin"
              ln -sf "$(which cargo-zigbuild)" "$_zigbuild_bin/x86_64-w64-mingw32-dlltool"
              export PATH="$PWD/$_zigbuild_bin:$PATH"
            '';
          };
        };
    };
}
