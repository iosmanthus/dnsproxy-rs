{
  description = "A simple dnsproxy written in Rust";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (
        system:
          let
            pkgs = nixpkgs.legacyPackages.${system};
          in
            {
              devShell = pkgs.mkShell {
                hardeningDisable = [ "all" ];
                buildInputs = with pkgs;[ git rustup gcc ];
              };
            }
      );
}
