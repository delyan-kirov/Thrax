{
  description = "Thrax for Windows: cross-compile from Linux with MinGW-w64, run under Wine";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      win = pkgs.pkgsCross.mingwW64;
      cc = win.stdenv.cc;
      triple = "x86_64-pc-windows-gnu";
      # The DLLs a built executable loads besides the system ones: libffi, and
      # the MinGW runtime (winpthreads, mcfgthread, libgcc). Wine finds them
      # through WINEPATH.
      dllPath = pkgs.lib.concatStringsSep ";" [
        "${win.libffi}/bin"
        "${win.windows.pthreads}/bin"
        "${win.windows.mcfgthreads}/bin"
        "${cc.cc}/x86_64-w64-mingw32/lib"
      ];

      # The corpus on both Windows engines, under Wine. Wine cannot wait on the
      # Unix gcc that `thrax.exe build` would launch, so the native leg goes
      # through `emit-c` and the cross gcc instead; the C is the same.
      wineCorpus = pkgs.writeShellScriptBin "thrax-wine-corpus" ''
        set -eu
        cargo build -p thrax
        t=$CARGO_TARGET_DIR/${triple}/debug
        out=$CARGO_TARGET_DIR/corpus
        mkdir -p "$out"
        wine "$t/thrax.exe" run tests/MAIN.thx > "$out/interp.txt"
        wine "$t/thrax.exe" emit-c tests/MAIN.thx > "$out/MAIN.c"
        x86_64-w64-mingw32-cc -O2 "$out/MAIN.c" -o "$out/MAIN.exe" -lm -lpthread -L"$t" -lthrax
        cp "$t/thrax.dll" "$out/"
        wine "$out/MAIN.exe" > "$out/native.txt"
        cat "$out/native.txt"
        cmp "$out/interp.txt" "$out/native.txt"
        echo "thrax-wine-corpus: OK (engines agree)"
      '';
    in
    {
      # Linux is the development platform; this shell builds and tests the
      # Windows executables from it. `cargo build` and `cargo test` produce
      # x86_64-pc-windows-gnu binaries, and cargo runs them under Wine.
      # `thrax-wine-corpus` runs the corpus on both engines. `cargo test` runs
      # too, except the tests that compile C at run time: under Wine a Windows
      # process cannot wait on the Unix gcc. Windows CI runs those natively.
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          wineCorpus
          win.buildPackages.rustc
          win.buildPackages.cargo
          cc
          pkgs.wineWow64Packages.stable
          pkgs.git
        ];

        shellHook = ''
          root=$(git rev-parse --show-toplevel 2>/dev/null || echo "$PWD")
          export CARGO_BUILD_TARGET=${triple}
          # Its own directory: the cross rustc is another version than the dev
          # shell's, and sharing target/ would rebuild one for the other.
          export CARGO_TARGET_DIR=$root/target/windows
          export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-cc
          export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine
          # Rust's std links winpthreads statically.
          export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-L native=${win.windows.pthreads}/lib"
          # Native programs with `@par` include <pthread.h>, which the MinGW gcc
          # in nixpkgs does not carry on its own search path.
          export NIX_CFLAGS_COMPILE_x86_64_w64_mingw32="-isystem ${win.windows.pthreads}/include"
          export NIX_LDFLAGS_x86_64_w64_mingw32="-L${win.windows.pthreads}/lib"
          export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-cc
          export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
          export LIBFFI=${win.libffi}
          export LIBFFI_DEV=${win.libffi.dev}
          export WINEPATH="${dllPath}"
          export WINEDEBUG=-all
          # A prefix of its own, created on first entry: Wine's default one may
          # not exist yet, and a half-initialised prefix aborts on missing APIs.
          export WINEPREFIX=$root/target/windows/wineprefix
          [ -d "$WINEPREFIX" ] || wineboot -i >/dev/null 2>&1
        '';
      };
    };
}
