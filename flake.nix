{
  description = "zsh-patina: A blazingly fast Zsh plugin performing syntax highlighting of your command line while you type";
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };
  outputs =
    { self, nixpkgs }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = nixpkgs.legacyPackages;
    in {
      packages = forAllSystems (system:  let
        pkgs = import nixpkgs {inherit system;};
        manifest = (pkgs.lib.importTOML ./Cargo.toml).package;
      in {
        default = pkgs.lib.warn
          ''
            zsh-patina has been added to nixpkgs-26.05 and nixpkgs-unstable.
            The flake.nix in the zsh-patina repository is deprecated and will be removed.
            Please change your configuration to use pkgs.zsh-patina from nixpkgs.
            If unclear, consult the README of zsh-patina for instructions.
          ''
          (pkgs.rustPlatform.buildRustPackage rec {
            pname = manifest.name;
            version = manifest.version;
            cargoLock.lockFile = ./Cargo.lock;
            src = pkgs.lib.cleanSource ./.;
          });
      });
    };
}
