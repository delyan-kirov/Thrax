{
  description = "Thrax on macOS: build and test the compiler natively";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # nixpkgs no longer builds for Intel Macs.
      system = "aarch64-darwin";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      # Linux is the development platform (the root flake); this shell is the
      # smaller set that builds and tests on a Mac: no valgrind (Linux only), no
      # emscripten, no editor tooling. The stdenv's clang is the `cc` that both
      # the build scripts and `thrax build` call.
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.libffi
          pkgs.git
          pkgs.bison
        ];

        shellHook = ''
          # Read by crates/interpreter/build.rs to link libffi for `@extern`.
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}
          export THRAX_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || echo "$PWD")
        '';
      };
    };
}
