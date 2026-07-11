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
      # Enter with `nix develop ./external`, then run `build ffi-rebuild` from
      # the repo root. This shell carries the heavy, rarely-needed toolchain to
      # bootstrap and build libffi from the git subtree (the fork ships no
      # generated `configure`), deliberately kept OUT of the main dev shell so
      # the everyday `nix develop` stays lean and fast.
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
