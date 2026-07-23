{
  description = "C++ dev environment for Thrax";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      mingwPkgs = pkgs.pkgsCross.mingwW64;
      wasiPkgs = pkgs.pkgsCross.wasi32; # clang + wasi-libc for wasm32-wasi
      # nix ships only prefixed binutils (wasm32-unknown-wasi-wasm-ld) but
      # clang invokes the linker as bare `wasm-ld`, so give it one on PATH.
      wasiLd = pkgs.runCommand "wasi-ld-shim" { } ''
        mkdir -p $out/bin
        ln -s ${wasiPkgs.stdenv.cc.bintools.bintools}/bin/wasm32-unknown-wasi-wasm-ld \
          $out/bin/wasm-ld
      '';
      wasiClang = pkgs.writeShellScriptBin "wasi-clang" ''
        export PATH=${wasiLd}/bin:$PATH
        exec ${wasiPkgs.stdenv.cc}/bin/wasm32-unknown-wasi-clang "$@"
      '';
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        # -O0 debug builds trip glibc's _FORTIFY_SOURCE warning (which needs -O);
        # disable that hardening so the makefile stays free of workaround flags.
        hardeningDisable = [ "fortify" ];

        buildInputs = [
          # Tools
          pkgs.clang
          pkgs.clang-tools
          pkgs.gcc
          pkgs.git
          pkgs.valgrind

          # Prebuilt deps (consumed via $LIBFFI / $RAYLIB in shellHook)
          pkgs.libffi

          pkgs.tokei
          pkgs.bear # compile_commands.json via `build compile-commands`
          pkgs.bison # grammar spec + conflict check (see grammar/)
          pkgs.wasmtime # runs wasm32-wasi executables (`--target=wasm32-wasi`)
          pkgs.emscripten # `build wasm`: the compiler itself to wasm (browser)
          pkgs.nodejs # runs the emscripten output headlessly (tests, CI)
          # mingwPkgs.stdenv.cc
          # should be enabled manually to check windows build
          # pkgs.wineWow64Packages.stable
        ];

        shellHook = ''
          export RAYLIB=${pkgs.raylib}
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}
          export LIBC=${pkgs.libc}
          export WASI_CC=${wasiClang}/bin/wasi-clang

          # Bootstrap the build program (it self-rebuilds thereafter) and put it
          # + the built binaries on PATH. nix is an accelerator here, not a
          # requirement: the same `build.cpp` builds without nix (see README).
          export THRAX_ROOT=$PWD
          [ -x ./build ] || clang++ -std=c++23 -Iutilities build.cpp utilities/UTxIO.cpp utilities/AR.cpp -o build
          export PATH=$PWD:$PWD/artifacts:$PATH
        '';
      };
    };
}
