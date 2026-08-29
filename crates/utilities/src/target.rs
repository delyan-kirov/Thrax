//! The compilation target as data: the single source of platform truth for one
//! compilation (the Rust port of the C++ `TG` layer).
//!
//! HOST (where a stage runs) and TARGET (what the program is compiled for) are
//! distinct values. [`Target::host`] is the ONE place allowed to consult the
//! build's `cfg!` platform flags; every other consumer takes a [`Target`] and
//! reads its policy from these methods rather than from its own conditionals.
//! The interpreter is defined to run programs for the host, so it uses
//! `Target::host()`; the C backend takes whatever target it is asked to emit
//! for, so a cross build (e.g. `wasm32`) reflects that target throughout.
//!
//! Everything platform-dependent lives here: the word size ([`Target::ptr_bits`],
//! which drives what `Int`/`Nat` alias to and the literal bounds), the real
//! width, how a symbolic `@extern` library name resolves to a loadable or
//! link name ([`Target::soname`], [`Target::link_flag`]), and how the host
//! invokes a C compiler for the target ([`toolchain`]).

/// The operating system a target runs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Macos,
    Windows,
    Wasi,
}

/// The processor architecture a target runs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86_64,
    Aarch64,
    X86,
    Arm,
    Wasm32,
}

/// A compilation target: an OS paired with an architecture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

impl Target {
    /// The pointer width in bits: the target's machine word.
    pub const fn ptr_bits(self) -> u32 {
        match self.arch {
            Arch::X86_64 | Arch::Aarch64 => 64,
            Arch::X86 | Arch::Arm | Arch::Wasm32 => 32,
        }
    }

    /// The width `Real`/`Real64` occupy. Every supported target is IEEE double.
    pub const fn real_bits(self) -> u32 {
        64
    }

    /// The largest integer LITERAL that fits this target's word. On a 32-bit
    /// word an unsigned 32-bit constant is admissible; on 64-bit the signed
    /// maximum. Held in `i128` so both bounds are representable exactly.
    pub const fn lit_max(self) -> i128 {
        if self.ptr_bits() == 32 {
            0xffff_ffff
        } else {
            0x7fff_ffff_ffff_ffff
        }
    }

    /// The largest signed `Int` value on this target.
    pub const fn int_max(self) -> i128 {
        if self.ptr_bits() == 32 {
            0x7fff_ffff
        } else {
            0x7fff_ffff_ffff_ffff
        }
    }

    /// The smallest signed `Int` value on this target.
    pub const fn int_min(self) -> i128 {
        if self.ptr_bits() == 32 {
            -0x8000_0000
        } else {
            -0x8000_0000_0000_0000
        }
    }

    /// The canonical sigil type `Int`/`Nat` alias to on this target: the word.
    pub const fn int_ty(self) -> &'static str {
        if self.ptr_bits() == 32 {
            "@int32"
        } else {
            "@int64"
        }
    }

    /// The canonical sigil type `Nat` aliases to on this target.
    pub const fn nat_ty(self) -> &'static str {
        if self.ptr_bits() == 32 {
            "@nat32"
        } else {
            "@nat64"
        }
    }

    /// The C library's runtime name on this target. Source never spells this:
    /// `@extern` names libraries symbolically (`"libc"`) and consumers resolve
    /// through [`Target::soname`] at the last moment.
    pub const fn libc_soname(self) -> &'static str {
        match self.os {
            Os::Linux => "libc.so.6",
            Os::Macos => "libSystem.B.dylib",
            Os::Windows => "msvcrt.dll",
            Os::Wasi => "",
        }
    }

    /// The math library's runtime name. On Linux libm is its own soname even
    /// where glibc folded the symbols into libc (Nix still ships them only in
    /// libm.so.6); elsewhere the C library carries the math symbols.
    pub const fn libm_soname(self) -> &'static str {
        match self.os {
            Os::Linux => "libm.so.6",
            _ => self.libc_soname(),
        }
    }

    /// Whether the target can load shared libraries at runtime. Wasm links
    /// statically only.
    pub const fn has_dlopen(self) -> bool {
        !matches!(self.os, Os::Wasi)
    }

    /// Resolve a symbolic `@extern` library name to this target's loadable name.
    /// `"libc"`/`"libm"` map to the known C/math libraries; a name containing
    /// `.` or `/` is an explicit soname/path and passes through verbatim;
    /// anything else gets the platform's conventional decoration (`raylib` ->
    /// `libraylib.so` / `libraylib.dylib` / `raylib.dll`, a leading `lib` not
    /// doubled).
    pub fn soname(self, lib: &str) -> String {
        match lib {
            "libc" | "c" => return self.libc_soname().to_string(),
            "libm" | "m" => return self.libm_soname().to_string(),
            _ => {}
        }
        if lib.contains('.') || lib.contains('/') {
            return lib.to_string();
        }
        match self.os {
            Os::Linux => decorate(lib, "lib", ".so"),
            Os::Macos => decorate(lib, "lib", ".dylib"),
            Os::Windows => format!("{lib}.dll"),
            Os::Wasi => lib.to_string(),
        }
    }

    /// The linker flag that satisfies a symbolic `@extern` library at link time
    /// (the native backend links rather than dlopens). An empty name (a symbol
    /// the engine itself provides, e.g. the `runtime.c` intrinsics) and `libc`
    /// are implicit and need none; `libm` becomes `-lm`; an explicit path/soname
    /// is passed verbatim; anything else becomes `-l<name>` (a leading `lib`
    /// stripped).
    pub fn link_flag(self, lib: &str) -> Option<String> {
        match lib {
            "" | "libc" | "c" => None,
            "libm" | "m" => Some("-lm".to_string()),
            _ if lib.contains('/') || lib.contains('.') => Some(lib.to_string()),
            _ => Some(format!("-l{}", lib.strip_prefix("lib").unwrap_or(lib))),
        }
    }

    /// The architecture's canonical spelling (`"x86_64"`, `"wasm32"`, ...).
    pub const fn arch_name(self) -> &'static str {
        match self.arch {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
            Arch::X86 => "x86",
            Arch::Arm => "arm",
            Arch::Wasm32 => "wasm32",
        }
    }

    /// The OS's canonical spelling (`"linux"`, `"wasi"`, ...).
    pub const fn os_name(self) -> &'static str {
        match self.os {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Wasi => "wasi",
        }
    }

    /// The `arch-os` name (`"x86_64-linux"`, `"wasm32-wasi"`), the inverse of
    /// [`Target::parse`].
    pub fn name(self) -> String {
        format!("{}-{}", self.arch_name(), self.os_name())
    }

    /// The machine this compiler binary runs on. The one sanctioned use of the
    /// build's `cfg!` platform flags.
    pub const fn host() -> Target {
        let os = if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "wasi") {
            Os::Wasi
        } else {
            Os::Linux
        };
        let arch = if cfg!(target_arch = "x86_64") {
            Arch::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else if cfg!(target_arch = "wasm32") {
            Arch::Wasm32
        } else if cfg!(target_arch = "x86") {
            Arch::X86
        } else if cfg!(target_arch = "arm") {
            Arch::Arm
        } else {
            Arch::X86_64
        };
        Target { os, arch }
    }

    /// Parse a `--target=` spelling (`"x86_64-linux"`, `"wasm32-wasi"`, ...).
    pub fn parse(s: &str) -> Option<Target> {
        const OSES: [Os; 4] = [Os::Linux, Os::Macos, Os::Windows, Os::Wasi];
        const ARCHES: [Arch; 5] = [
            Arch::X86_64,
            Arch::Aarch64,
            Arch::X86,
            Arch::Arm,
            Arch::Wasm32,
        ];
        for os in OSES {
            for arch in ARCHES {
                let t = Target { os, arch };
                if t.name() == s {
                    return Some(t);
                }
            }
        }
        None
    }
}

fn decorate(lib: &str, prefix: &str, suffix: &str) -> String {
    if lib.starts_with(prefix) {
        format!("{lib}{suffix}")
    } else {
        format!("{prefix}{lib}{suffix}")
    }
}

/// How the host invokes a C compiler for a target: the compiler call as data.
/// The host target uses the system `cc`; a wasm target uses `emcc` (overridable
/// via `THRAX_WASM_CC`), so tool LOCATIONS stay out of the compiler the way
/// `@extern` library locations do. An empty `cc` means no toolchain was found
/// and `hint` says what to provide.
#[derive(Clone, Debug)]
pub struct Toolchain {
    /// C compiler driver; empty means none is available for the target.
    pub cc: String,
    /// When `cc` is empty: how to provide one.
    pub hint: String,
    /// Flags always passed (optimization level, wasm layout, ...).
    pub cflags: Vec<String>,
    /// Wraps execution of the built program (`"node"`, `"wasmtime"`); empty
    /// runs the executable directly.
    pub runner: String,
    /// Output suffix: `""` native, `".js"` for the emscripten wasm loader.
    pub exe_suffix: String,
    /// Whether the linker understands `-Wl,-rpath` (has runtime loading).
    pub rpath: bool,
}

/// The toolchain for building `target` on this host.
pub fn toolchain(target: Target) -> Toolchain {
    let mut tc = Toolchain {
        cc: String::new(),
        hint: String::new(),
        cflags: vec!["-O2".to_string()],
        runner: String::new(),
        exe_suffix: String::new(),
        rpath: true,
    };
    if target == Target::host() {
        tc.cc = "cc".to_string();
        return tc;
    }
    if target.os == Os::Wasi && target.arch == Arch::Wasm32 {
        tc.cc = std::env::var("THRAX_WASM_CC").unwrap_or_else(|_| "emcc".to_string());
        tc.hint =
            "no wasm C compiler found: install emscripten (emcc) or set THRAX_WASM_CC".to_string();
        tc.runner = "node".to_string();
        // emcc writes <name>.js plus a sibling <name>.wasm; node runs the .js.
        tc.exe_suffix = ".js".to_string();
        tc.rpath = false;
        return tc;
    }
    tc.cflags.clear();
    tc.hint = format!(
        "cross-compilation to '{}' is not supported yet (host is {})",
        target.name(),
        Target::host().name()
    );
    tc
}
