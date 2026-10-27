{
  lib,
  rustPlatform,
  makeWrapper,
  typst,
  d2,
  plantuml,
  mermaid-cli,
}:
let
  cargoToml = lib.importTOML ../Cargo.toml;
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  version = cargoToml.package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../themes
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];

  # Tests call external diagram tools and need a writable cache.
  doCheck = false;

  # Diagram and PDF backends find these tools through PATH at run time.
  # Tools on the user's PATH take priority.
  postInstall = ''
    wrapProgram $out/bin/docbake \
      --suffix PATH : ${
        lib.makeBinPath [
          typst
          d2
          plantuml
          mermaid-cli
        ]
      }
  '';

  meta = {
    description = "Multi-format Markdown documentation compiler";
    mainProgram = "docbake";
  };
}
