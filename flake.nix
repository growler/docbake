{
  description = "Multi-format Markdown documentation compiler";

  inputs = {
    nixpkgs.url = "nixpkgs";
  };

  outputs =
    { nixpkgs, ... }:
    let
      overlays.default = import ./nix/overlay.nix;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ overlays.default ];
            }
          )
        );
    in
    {
      inherit overlays;
      packages = forAllSystems (pkgs: {
        docbake = pkgs.docbake;
        default = pkgs.docbake;
      });
    };
}
