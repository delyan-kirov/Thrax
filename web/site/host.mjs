// The host side of the Thrax playground's `@extern` bridge.
//
// The compiler runs as WebAssembly with no libc, so a Thrax `@extern` resolves
// against a registry the embedder owns rather than a C symbol table. This module
// implements the generic call trampoline the interpreter speaks (see
// `run_extern` in crates/interpreter/src/machine/data.rs) and exposes a registry
// you extend with `register(name, fn)`. The wasm module knows none of these
// names; adding a host function is purely a page concern.
//
// `createHost()` returns:
//   imports          the import object to instantiate the wasm module with
//   bind(exports)    hand it the instance's exports once instantiated
//   register(n, fn)  add a host function callable from Thrax as `@extern ... "n"`
//   compile(src,mode) run the compiler (0 run, 1 C, 2 IR, 3 AST); returns the
//                     text the program printed followed by the staged result

export function createHost() {
  const dec = new TextDecoder(), enc = new TextEncoder();
  let wasm = null;

  // Whatever the running program prints through a host function, per compile.
  let printed = "";

  // The interpreter runs to completion before the browser repaints, so a program
  // that changes color in a loop would only ever show its last color. Instead we
  // COLLECT the colors it asks for during the run and play them back on a timer
  // afterwards (see `compile`). That is what makes the loop visible.
  const hasDom = typeof document !== "undefined";
  let colors = [];
  let colorTimer = null;
  let playbackDelay = 180;   // ms between frames, settable from Thrax via `delay`

  function playColors() {
    if (colorTimer) { clearInterval(colorTimer); colorTimer = null; }
    if (!hasDom || colors.length === 0) return;
    let i = 0;
    const show = () => document.documentElement.style.setProperty("--card", colors[i++ % colors.length]);
    show();
    if (colors.length > 1) colorTimer = setInterval(show, playbackDelay);
  }

  // The registry: `@extern "WASM"` symbol -> JavaScript function. A function may
  // take int (BigInt) / real (number) / string arguments and return one of
  // those, or nothing. The default `print` writes a line to the output pane.
  const registry = {
    print(s) { printed += s + "\n"; },
    change_color(color) { colors.push(color); },
    // A real random integer in [0, n), straight from JavaScript.
    random(n) { return Math.floor(Math.random() * Number(n)); },
    // Set the delay (ms) between color frames in the playback above.
    delay(ms) { playbackDelay = Number(ms); },
  };

  const mem = () => new Uint8Array(wasm.memory.buffer);
  const view = () => new DataView(wasm.memory.buffer);

  // Decode the interpreter's kind-tagged argument buffer into JS values.
  function decodeArgs(ptr, len) {
    const m = mem(), v = view();
    const args = [];
    let o = ptr;
    const end = ptr + len;
    while (o < end) {
      const kind = m[o++];
      if (kind === 0) { args.push(undefined); }
      else if (kind === 1) { args.push(v.getBigInt64(o, true)); o += 8; }
      else if (kind === 2) { args.push(v.getFloat64(o, true)); o += 8; }
      else if (kind === 3) {
        const n = v.getUint32(o, true); o += 4;
        args.push(dec.decode(m.subarray(o, o + n))); o += n;
      } else break;
    }
    return args;
  }

  // The result the most recent call produced, read back by the typed getters.
  let retInt = 0n, retReal = 0, retBytes = null;

  const env = {
    thx_host_call(symPtr, symLen, argsPtr, argsLen) {
      const name = dec.decode(mem().subarray(symPtr, symPtr + symLen));
      const fn = registry[name];
      if (typeof fn !== "function") return -1;
      const r = fn(...decodeArgs(argsPtr, argsLen));
      if (r === undefined || r === null) return 0;
      if (typeof r === "bigint") { retInt = r; return 1; }
      if (typeof r === "number") {
        if (Number.isInteger(r)) { retInt = BigInt(r); return 1; }
        retReal = r; return 2;
      }
      if (typeof r === "string") { retBytes = enc.encode(r); return 3; }
      return 0;
    },
    thx_host_ret_int() { return retInt; },
    thx_host_ret_real() { return retReal; },
    thx_host_ret_len() { return retBytes ? retBytes.length : 0; },
    thx_host_ret_copy(dst) { if (retBytes) mem().set(retBytes, dst); },
  };

  // Serve the real host imports from `env`; answer any other import with a no-op
  // so an unrelated import can never block instantiation.
  const imports = new Proxy({}, {
    get: (_t, module) => new Proxy(module === "env" ? env : {}, {
      get: (target, name) => (name in target ? target[name] : () => 0),
    }),
  });

  function compile(src, mode) {
    printed = "";
    // Every run starts from the default color, then plays back whatever it asks
    // for. A run that paints nothing (the flag off) leaves the default alone.
    colors = [];
    playbackDelay = 180;
    if (colorTimer) { clearInterval(colorTimer); colorTimer = null; }
    if (hasDom) document.documentElement.style.removeProperty("--card");
    const bytes = enc.encode(src);
    const ptr = wasm.thx_alloc(bytes.length);
    mem().set(bytes, ptr);
    wasm.thx_compile(ptr, bytes.length, mode);
    const op = wasm.thx_out_ptr(), ol = wasm.thx_out_len();
    const staged = dec.decode(mem().subarray(op, op + ol));
    playColors();
    return printed + staged;
  }

  return {
    imports,
    bind: (exports) => { wasm = exports; },
    register: (name, fn) => { registry[name] = fn; },
    compile,
  };
}
