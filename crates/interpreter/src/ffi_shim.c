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
    K_FLOAT, K_DOUBLE, K_PTR,
    K_STRUCT
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

#define MAX_ARGS   32
#define MAX_LEAVES 32

/* Fill `agg` (and its `elems` array, room for MAX_LEAVES+1) as an aggregate
 * ffi_type over `n` leaf kinds. libffi computes size/alignment/offsets during
 * `ffi_prep_cif`, matching the standard C layout of a struct of these leaves. */
static int build_struct(ffi_type *agg, ffi_type **elems, const int *leaves, int n) {
    if (n > MAX_LEAVES) return -1;
    for (int i = 0; i < n; i++) elems[i] = ty(leaves[i]);
    elems[n] = NULL;
    agg->size = 0;
    agg->alignment = 0;
    agg->type = FFI_TYPE_STRUCT;
    agg->elements = elems;
    return 0;
}

/* Prepare and perform one call, supporting by-value struct arguments and a
 * by-value struct return. For argument i:
 *   kinds[i] != K_STRUCT : a scalar of that kind; avalues[i] -> its value cell.
 *   kinds[i] == K_STRUCT : a struct whose leaf kinds are
 *       leaves[leaf_off[i] .. leaf_off[i] + leaf_len[i]];
 *       avalues[i] -> the struct's bytes.
 * The return is a scalar `ret_kind`, or (ret_kind == K_STRUCT) a struct built
 * from `ret_leaves[0..ret_nleaves]`. `rvalue` must have room for the result
 * (>= struct size, or `sizeof(ffi_arg)`), 8-byte aligned. Returns 0 on success. */
int thx_ffi_call_x(void *fn, unsigned nargs,
                   const int *kinds,
                   const int *leaf_off, const int *leaf_len, const int *leaves,
                   int ret_kind, const int *ret_leaves, int ret_nleaves,
                   void **avalues, void *rvalue) {
    ffi_cif cif;
    ffi_type *atypes[MAX_ARGS];
    ffi_type struct_store[MAX_ARGS];
    ffi_type *elem_store[MAX_ARGS][MAX_LEAVES + 1];
    if (nargs > MAX_ARGS) return 2;
    for (unsigned i = 0; i < nargs; i++) {
        if (kinds[i] == K_STRUCT) {
            if (build_struct(&struct_store[i], elem_store[i],
                             &leaves[leaf_off[i]], leaf_len[i]) != 0)
                return 3;
            atypes[i] = &struct_store[i];
        } else {
            atypes[i] = ty(kinds[i]);
        }
    }

    ffi_type ret_struct;
    ffi_type *ret_elems[MAX_LEAVES + 1];
    ffi_type *rtype;
    if (ret_kind == K_STRUCT) {
        if (build_struct(&ret_struct, ret_elems, ret_leaves, ret_nleaves) != 0)
            return 3;
        rtype = &ret_struct;
    } else {
        rtype = ty(ret_kind);
    }

    if (ffi_prep_cif(&cif, FFI_DEFAULT_ABI, nargs, rtype,
                     nargs ? atypes : NULL) != FFI_OK)
        return 1;
    ffi_call(&cif, FFI_FN(fn), rvalue, nargs ? avalues : NULL);
    return 0;
}
