{
  description = "Dev environment for Thrax";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
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

      # The compiler, built by the Rust workspace. The interpreter's build.rs
      # links libffi (for `@extern`) against $LIBFFI/$LIBFFI_DEV and compiles a
      # small C shim, so libffi + a C toolchain (cc/ar) are build inputs; there
      # are no external Rust crates to fetch, so the build is offline.
      thrax = pkgs.stdenv.mkDerivation {
        pname = "thrax";
        version = "0.1.0";
        src = self;

        nativeBuildInputs = [
          pkgs.cargo
          pkgs.rustc
          pkgs.gcc
        ];
        buildInputs = [ pkgs.libffi ];

        buildPhase = ''
          runHook preBuild
          export HOME=$TMPDIR
          export CARGO_HOME=$TMPDIR/.cargo
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}
          cargo build --release --offline -p thrax
          runHook postBuild
        '';

        installPhase = ''
          runHook preInstall
          install -Dm755 target/release/thrax $out/bin/thrax
          runHook postInstall
        '';

        meta = {
          description = "The Thrax compiler and interpreter";
          mainProgram = "thrax";
        };
      };
    in
    {
      packages.${system} = {
        inherit thrax;
        default = thrax;
      };

      apps.${system}.default = {
        type = "app";
        program = "${thrax}/bin/thrax";
      };

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

          # Rust toolchain for the in-progress rewrite (no external crates; a
          # bare rustc + cargo is all the workspace needs). rustfmt/clippy/
          # rust-analyzer are dev ergonomics only.
          pkgs.rustc
          pkgs.cargo
          pkgs.rustfmt
          pkgs.clippy
          pkgs.rust-analyzer
          # wasm-ld for building the Rust compiler crates to wasm32-unknown-
          # unknown (the browser playground); nixpkgs rustc ships that target's
          # std but not a bundled rust-lld, so provide the linker on PATH.
          pkgs.lld

          # Prebuilt deps (consumed via $LIBFFI / $RAYLIB in shellHook)
          pkgs.libffi

          pkgs.tokei
          pkgs.bear # compile_commands.json via `build compile-commands`
          pkgs.bison # grammar spec + conflict check (see grammar/)
          pkgs.wasmtime # runs wasm32-wasi executables (`--target=wasm32-wasi`)
          pkgs.emscripten # `build wasm`: the compiler itself to wasm (browser)
          pkgs.nodejs # runs the emscripten output headlessly (tests, CI)
          pkgs.zig # `build win` / `win-test`: cross-compiles Thrax to Windows
          pkgs.wineWow64Packages.stable # runs the resulting .exe headlessly
        ];

        shellHook = ''
          export RAYLIB=${pkgs.raylib}
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}
          export LIBC=${pkgs.libc}
          export WASI_CC=${wasiClang}/bin/wasi-clang

          # The workspace builds with a bare `cargo build`; the interpreter's
          # build.rs builds the vendored external/libffi from source (needs the
          # cc/make/ar already on PATH here).
          export THRAX_ROOT=$PWD
        '';
      };
    };
}
