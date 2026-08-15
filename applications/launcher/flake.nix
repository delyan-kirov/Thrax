{
  description = "Thrax program launcher -- a raylib app using the C-struct FFI";

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
          pkgs.raylib # the library the launcher binds via @extern

          # Runtime deps of libraylib.so, so a window can open.
          pkgs.libGL
          pkgs.xorg.libX11
          pkgs.xorg.libXcursor
          pkgs.xorg.libXi
          pkgs.xorg.libXinerama
          pkgs.xorg.libXrandr
        ];

        # Make this directory self-contained: `bin/libraylib.so` is the library
        # the `@extern` paths name (relative to here), and `library/` is the Thrax
        # standard library the interpreter resolves `CORE` from. The `thrax`
        # binary comes from the repo build (see README).
        shellHook = ''
          export RAYLIB=${pkgs.raylib}
          mkdir -p bin
          ln -sf ${pkgs.raylib}/lib/libraylib.so bin/libraylib.so
          ln -sf ../../library library
          echo "launcher shell ready. Build thrax once in the repo, then:"
          echo "    thrax run MAIN.thx      # or: ../../target/debug/thrax run MAIN.thx"
        '';
      };
    };
}
