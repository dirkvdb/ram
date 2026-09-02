{
  description = "A small Linux memory overview CLI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;

      mkRam =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "ram";
          version = "0.1.0";

          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          strictDeps = true;

          meta = {
            description = "A small Linux memory overview CLI";
            license = pkgs.lib.licenses.mit;
            mainProgram = "ram";
            platforms = pkgs.lib.platforms.linux;
          };
        };
    in
    {
      overlays.default = final: _prev: {
        ram = mkRam final;
      };

      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          ram = mkRam pkgs;
        in
        {
          inherit ram;
          default = ram;
        }
      );

      apps = forAllSystems (system: {
        ram = {
          type = "app";
          program = nixpkgs.lib.getExe self.packages.${system}.ram;
          meta.description = "Show an overview of Linux memory usage";
        };
        default = self.apps.${system}.ram;
      });

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) ram;
      });

      formatter = forAllSystems (system: (import nixpkgs { inherit system; }).nixfmt);
    };
}
