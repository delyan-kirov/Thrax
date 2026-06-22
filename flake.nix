{
  description = "C++ dev environment for Thrax";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      mingwPkgs = pkgs.pkgsCross.mingwW64;
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        # -O0 debug builds trip glibc's _FORTIFY_SOURCE warning (which needs -O);
        # disable that hardening so the makefile stays free of workaround flags.
        hardeningDisable = [ "fortify" ];

        buildInputs = [
          # Tools
          pkgs.clang
          pkgs.gcc
          pkgs.gnumake
          pkgs.git
          pkgs.valgrind

          # Prebuilt deps (consumed via $LIBFFI / $RAYLIB in shellHook)
          pkgs.libffi
          # pkgs.raylib

          # X11 support for raylib
          # pkgs.libX11
          # pkgs.libX11.dev
          # pkgs.libXcursor
          # pkgs.libXi
          # pkgs.libXinerama
          # pkgs.libXrandr

          # extra
          pkgs.tokei
          # mingwPkgs.stdenv.cc
          # should be enabled manually to check windows build
          # pkgs.wineWow64Packages.stable
        ];

        shellHook = ''
          export RAYLIB=${pkgs.raylib}
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}
        '';
      };
    };
}
