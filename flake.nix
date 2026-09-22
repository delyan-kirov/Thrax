{
  description = "Dev environment for Thrax";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # A `thrax` on PATH inside the dev shell that always runs the current
      # source: it builds the workspace binary (incrementally, silent when up
      # to date) and execs it. Available before the binary exists, so a fresh
      # clone can `thrax run` immediately. THRAX_ROOT (set in shellHook) locates
      # the workspace; "$@" is forwarded with the caller's cwd intact so
      # relative source paths and the no-argument MAIN.thx default resolve there.
      thraxDev = pkgs.writeShellScriptBin "thrax" ''
        root="''${THRAX_ROOT:-$PWD}"
        cargo build --quiet --manifest-path "$root/Cargo.toml" -p thrax 1>&2 || exit
        exec "$root/target/debug/thrax" "$@"
      '';

      # The task runner: `just`, but the tasks are Thrax (see tools/thxdo.thx).
      # `thxdo <cmd>` runs a NATIVE binary compiled from tools/thxdo.thx, so
      # `thxdo` / `thxdo help` are instant. The binary is (re)built only when its
      # source or the compiler changed; otherwise this is a bare exec. New tasks
      # are arms in tools/thxdo.thx, no wrapper change needed.
      thxdoDev = pkgs.writeShellScriptBin "thxdo" ''
        root="''${THRAX_ROOT:-$PWD}"
        cd "$root"
        export THRAX_BIN="$root/target/debug/thrax"
        bin="$root/tools/thrax-out/thxdo"
        src="$root/tools/thxdo.thx"
        # Rebuild the task-runner binary when its source changed or is missing, or
        # when the compiler is absent (e.g. after `thxdo clean`). A mere compiler
        # recompile does NOT force a rebuild, so the common path stays a bare exec.
        if [ ! -x "$bin" ] || [ "$src" -nt "$bin" ] || [ ! -x "$THRAX_BIN" ]; then
          cargo build --quiet --manifest-path "$root/Cargo.toml" -p thrax 1>&2 || exit
          "$THRAX_BIN" build "$src" 1>&2 || exit
        fi
        exec "$bin" "$@"
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
        # disable that hardening so the C the build compiles stays warning-free.
        hardeningDisable = [ "fortify" ];

        buildInputs = [
          # The dev-shell `thrax`: builds the workspace binary on demand and
          # execs it, so `thrax run`/`build` work from a fresh checkout.
          thraxDev

          # The dev-shell `thxdo` task runner (tools/thxdo.thx), e.g. `thxdo readme`.
          thxdoDev

          # C toolchain for the interpreter's build.rs (vendored libffi + shim)
          # and the native backend's `cc`; valgrind for leak-hunting its output.
          pkgs.gcc
          pkgs.valgrind
          pkgs.git

          # Rust toolchain (no external crates; a bare rustc + cargo is all the
          # workspace needs). rustfmt/clippy/rust-analyzer are dev ergonomics.
          pkgs.rustc
          pkgs.cargo
          pkgs.rustfmt
          pkgs.clippy
          pkgs.rust-analyzer

          # libffi for `@extern` (consumed via $LIBFFI / $LIBFFI_DEV below).
          pkgs.libffi

          # `thrax --target=wasm32-wasi` uses emcc to build and node to run the
          # output (the target's runner in utilities::target). The web playground
          # carries its own toolchain (applications/web/flake.nix).
          pkgs.emscripten
          pkgs.nodejs

          pkgs.tokei # line counts
          pkgs.bison # conflict-check the grammar spec (documentation/thrax.y)
        ];

        shellHook = ''
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}

          # The workspace builds with a bare `cargo build`; the interpreter's
          # build.rs builds the vendored external/libffi from source (needs the
          # cc/make/ar the dev shell's stdenv already provides).
          export THRAX_ROOT=$PWD
        '';
      };
    };
}
