{
  description = "Dev environment for Ares";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # Builds the workspace binary before exec'ing it, so a fresh clone can
      # `ares run` before any binary exists. The caller's cwd is left alone so
      # relative source paths resolve against it.
      aresDev = pkgs.writeShellScriptBin "ares" ''
        root="''${ARES_ROOT:-$PWD}"
        cargo build --quiet --manifest-path "$root/Cargo.toml" -p ares 1>&2 || exit
        exec "$root/target/debug/ares" "$@"
      '';

      # Unlike aresDev, this cd's to the root, since the tasks assume they run
      # there.
      thraxDev = pkgs.writeShellScriptBin "thrax" ''
        root="''${ARES_ROOT:-$PWD}"
        cd "$root"
        export ARES_BIN="$root/target/debug/ares"
        bin="$root/tools/ares-out/THRAX"
        src="$root/tools/THRAX.ares"
        # A compiler recompile alone does not force a rebuild, so the common path
        # stays a bare exec. THRAX.ares imports the modules beside it.
        stale=
        for f in "$root"/tools/*.ares; do [ "$f" -nt "$bin" ] && stale=1; done
        if [ ! -x "$bin" ] || [ -n "$stale" ] || [ ! -x "$ARES_BIN" ]; then
          cargo build --quiet --manifest-path "$root/Cargo.toml" -p ares 1>&2 || exit
          "$ARES_BIN" build "$src" 1>&2 || exit
        fi
        exec "$bin" "$@"
      '';

      # The line counter in applications/zalmoxis, built natively when one of its
      # sources is newer than the binary. Like aresDev it leaves the cwd alone,
      # so it counts where it is run.
      zalmoxisDev = pkgs.writeShellScriptBin "zalmoxis" ''
        root="''${ARES_ROOT:-$PWD}"
        ares="$root/target/debug/ares"
        dir="$root/applications/zalmoxis"
        bin="$dir/ares-out/MAIN"
        if [ ! -f "$dir/MAIN.ares" ]; then
          echo "zalmoxis: $dir is missing (git submodule update --init applications)" 1>&2
          exit 1
        fi
        stale=
        for f in "$dir"/*.ares; do [ "$f" -nt "$bin" ] && stale=1; done
        if [ ! -x "$bin" ] || [ -n "$stale" ] || [ ! -x "$ares" ]; then
          cargo build --quiet --manifest-path "$root/Cargo.toml" -p ares 1>&2 || exit
          "$ares" build "$dir/MAIN.ares" 1>&2 || exit
        fi
        exec "$bin" "$@"
      '';

      # `--offline` holds because the workspace has no external crates.
      ares = pkgs.stdenv.mkDerivation {
        pname = "ares";
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
          cargo build --release --offline -p ares
          runHook postBuild
        '';

        # The standard library ships beside the binary, which finds it relative
        # to its own path: `$out/bin/ares` resolves `$out/library`. The tree is
        # relocatable and needs no environment variable. The compiler library
        # goes to `$out/lib`, where `ares build` finds it for a program that runs
        # the compiler (`@eval`, `@lex`, ...), with its C header in `$out/include`.
        installPhase = ''
          runHook preInstall
          install -Dm755 target/release/ares $out/bin/ares
          install -Dm755 target/release/libares.so $out/lib/libares.so
          install -Dm644 target/release/libares.a $out/lib/libares.a
          install -Dm644 crates/ares/include/ares.h $out/include/ares.h
          mkdir -p $out/library
          install -Dm644 library/*.ares $out/library/
          runHook postInstall
        '';

        meta = {
          description = "The Ares compiler and interpreter";
          mainProgram = "ares";
        };
      };
    in
    {
      packages.${system} = {
        inherit ares;
        default = ares;
      };

      apps.${system}.default = {
        type = "app";
        program = "${ares}/bin/ares";
      };

      devShells.${system}.default = pkgs.mkShell {
        # -O0 debug builds trip glibc's _FORTIFY_SOURCE warning, which needs -O.
        hardeningDisable = [ "fortify" ];

        buildInputs = [
          aresDev
          thraxDev
          zalmoxisDev

          # gcc compiles the C in the interpreter's build.rs and the native
          # backend's output; valgrind hunts leaks in that output.
          pkgs.gcc
          pkgs.valgrind
          pkgs.git

          pkgs.rustc
          pkgs.cargo
          pkgs.rustfmt
          pkgs.clippy
          pkgs.rust-analyzer

          pkgs.libffi

          # `ares --target=wasm32-wasi`
          pkgs.emscripten
          pkgs.nodejs

          pkgs.xdg-utils
          pkgs.bison

          # `thrax proofs`: the kernel's consistency proof in documentation/type-theory/consistency-proof/ (#481).
          pkgs.lean4
        ];

        shellHook = ''
          # Read by crates/interpreter/build.rs to link libffi for `@extern`.
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}

          # The directory `nix develop` was entered from: the workspace root, as
          # far as aresDev, thraxDev and `thrax tosdk` are concerned.
          export ARES_ROOT=$PWD

          # What `thrax hooks` sets: the git hooks of documentation/tooling/workflow.md.
          if [ -d .git ]; then
            git config core.hooksPath tools/hooks
            git config core.commentChar ';'
            git config push.autoSetupRemote true
            [ ! -e tools/editors/.git ] || git -C tools/editors config core.hooksPath hooks
          fi
        '';
      };
    };
}
