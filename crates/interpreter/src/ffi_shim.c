/* A thin C shim over libffi, so the Rust side never has to mirror libffi's
 * arch-specific `ffi_cif`/`ffi_type` layout or the value of `FFI_DEFAULT_ABI`.
 * Rust hands us type KIND codes and raw argument storage; we build the call
 * interface and perform it. Keep the kind codes in sync with `src/ffi.rs`. */
#include <ffi.h>
#include <stddef.h>

enum {
    K_VOID = 0,
    K_S8, K_S16, K_S32, K_S64,
    K_U8, K_U16, K_U32, K_U64,
    K_FLOAT, K_DOUBLE, K_PTR
};

static ffi_type *ty(int k) {
    switch (k) {
        case K_VOID:   return &ffi_type_void;
        case K_S8:     return &ffi_type_sint8;
        case K_S16:    return &ffi_type_sint16;
        case K_S32:    return &ffi_type_sint32;
        case K_S64:    return &ffi_type_sint64;
        case K_U8:     return &ffi_type_uint8;
        case K_U16:    return &ffi_type_uint16;
        case K_U32:    return &ffi_type_uint32;
        case K_U64:    return &ffi_type_uint64;
        case K_FLOAT:  return &ffi_type_float;
        case K_DOUBLE: return &ffi_type_double;
        default:       return &ffi_type_pointer;
    }
}

/* Prepare and perform one call. `avalues[i]` points at storage for argument i
 * (a `void*` cell for K_PTR, otherwise a cell holding the integer/float bits).
 * `rvalue` must have room for at least `sizeof(ffi_arg)` (8 bytes suffices on
 * every supported target). Returns 0 on success, nonzero if libffi refused. */
int thx_ffi_call(void *fn, unsigned nargs, const int *kinds, int ret_kind,
                 void **avalues, void *rvalue) {
    ffi_cif cif;
    ffi_type *atypes[64];
    if (nargs > 64) return 2;
    for (unsigned i = 0; i < nargs; i++) atypes[i] = ty(kinds[i]);
    if (ffi_prep_cif(&cif, FFI_DEFAULT_ABI, nargs, ty(ret_kind),
                     nargs ? atypes : NULL) != FFI_OK)
        return 1;
    ffi_call(&cif, FFI_FN(fn), rvalue, nargs ? avalues : NULL);
    return 0;
}
