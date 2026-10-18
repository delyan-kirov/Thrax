{
  description = "Dev shell for (re)building the vendored libffi subtree";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      # `thxdo build release` enters this shell to bootstrap and build libffi
      # from the git subtree (the fork ships no generated `configure`) when
      # autoreconf is not already on PATH. The heavy, rarely-needed toolchain is
      # kept OUT of the main dev shell so the everyday `nix develop` stays lean.
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.autoconf
          pkgs.automake
          pkgs.libtool
          pkgs.gnumake
          pkgs.gcc
          pkgs.texinfo # libffi's autogen/configure references makeinfo
        ];
      };
    };
}
