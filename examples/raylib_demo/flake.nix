{
  description = "Thrax raylib demo -- native-backend FFI example";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = [
          pkgs.raylib # the library the demo binds via @extern
          pkgs.clang # clang++, to bootstrap build.cpp
          pkgs.gcc # cc, to compile the emitted C

          # Runtime deps of libraylib.so. raylib's shared object already carries
          # nix rpaths to these, so listing them here is mainly for a display
          # backend to be present in the shell.
          pkgs.libGL
          pkgs.xorg.libX11
          pkgs.xorg.libXcursor
          pkgs.xorg.libXi
          pkgs.xorg.libXinerama
          pkgs.xorg.libXrandr
        ];

        # `./build` reads $RAYLIB to locate libraylib.so. Bootstrap the build
        # program (it self-rebuilds thereafter); it reuses the repo-root helpers
        # in ../../utilities, hence the include path.
        shellHook = ''
          export RAYLIB=${pkgs.raylib}
          [ -x ./build ] || clang++ -std=c++23 -O2 -I../../utilities build.cpp -o build
        '';
      };
    };
}
