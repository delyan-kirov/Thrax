{
  description = "Dev environment for Thrax";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # Builds the workspace binary before exec'ing it, so a fresh clone can
      # `thrax run` before any binary exists. The caller's cwd is left alone so
      # relative source paths resolve against it.
      thraxDev = pkgs.writeShellScriptBin "thrax" ''
        root="''${THRAX_ROOT:-$PWD}"
        cargo build --quiet --manifest-path "$root/Cargo.toml" -p thrax 1>&2 || exit
        exec "$root/target/debug/thrax" "$@"
      '';

      # Unlike thraxDev, this cd's to the root, since the tasks assume they run
      # there.
      thxdoDev = pkgs.writeShellScriptBin "thxdo" ''
        root="''${THRAX_ROOT:-$PWD}"
        cd "$root"
        export THRAX_BIN="$root/target/debug/thrax"
        bin="$root/tools/thrax-out/THXDO"
        src="$root/tools/THXDO.thx"
        # A compiler recompile alone does not force a rebuild, so the common path
        # stays a bare exec.
        if [ ! -x "$bin" ] || [ "$src" -nt "$bin" ] || [ ! -x "$THRAX_BIN" ]; then
          cargo build --quiet --manifest-path "$root/Cargo.toml" -p thrax 1>&2 || exit
          "$THRAX_BIN" build "$src" 1>&2 || exit
        fi
        exec "$bin" "$@"
      '';

      # `--offline` holds because the workspace has no external crates.
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
        # -O0 debug builds trip glibc's _FORTIFY_SOURCE warning, which needs -O.
        hardeningDisable = [ "fortify" ];

        buildInputs = [
          thraxDev
          thxdoDev

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

          # `thrax --target=wasm32-wasi`
          pkgs.emscripten
          pkgs.nodejs

          pkgs.tokei
          pkgs.xdg-utils
          pkgs.bison
        ];

        shellHook = ''
          # Read by crates/interpreter/build.rs to link libffi for `@extern`.
          export LIBFFI=${pkgs.libffi.out}
          export LIBFFI_DEV=${pkgs.libffi.dev}

          # The directory `nix develop` was entered from: the workspace root, as
          # far as thraxDev, thxdoDev and `thxdo tosdk` are concerned.
          export THRAX_ROOT=$PWD
        '';
      };
    };
}
