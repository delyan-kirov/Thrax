/* A thin C shim over libffi, so the Rust side never has to mirror libffi's
 * arch-specific `ffi_cif`/`ffi_type` layout or the value of `FFI_DEFAULT_ABI`.
 * Rust hands us type KIND codes and raw argument storage; we build the call
 * interface and perform it. Keep the kind codes in sync with `src/ffi.rs`. */
#include <ffi.h>
#include <stddef.h>
#include <stdlib.h>

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

/* Fill `agg` as an aggregate ffi_type over `n` leaf kinds, with `elems` (room
 * for n+1) as its NULL-terminated element list. libffi computes size, alignment
 * and offsets during `ffi_prep_cif`, matching the standard C layout of a struct
 * of these leaves. */
static void build_struct(ffi_type *agg, ffi_type **elems, const int *leaves, int n) {
    for (int i = 0; i < n; i++) elems[i] = ty(leaves[i]);
    elems[n] = NULL;
    agg->size = 0;
    agg->alignment = 0;
    agg->type = FFI_TYPE_STRUCT;
    agg->elements = elems;
}

/* A prepared call interface. The cif points into `atypes`, `aggs` and `elems`,
 * so the four live and die together. */
typedef struct {
    ffi_cif cif;
    ffi_type **atypes;
    ffi_type *aggs;
    ffi_type **elems;
} thx_prep_t;

void thx_ffi_prep_free(void *handle) {
    thx_prep_t *p = handle;
    if (p) {
        free(p->atypes);
        free(p->aggs);
        free(p->elems);
        free(p);
    }
}

/* Prepare the call interface of a signature, supporting by-value struct
 * arguments and a by-value struct return. For argument i:
 *   kinds[i] != K_STRUCT : a scalar of that kind.
 *   kinds[i] == K_STRUCT : a struct whose leaf kinds are
 *       leaves[leaf_off[i] .. leaf_off[i] + leaf_len[i]].
 * The return is a scalar `ret_kind`, or (ret_kind == K_STRUCT) a struct built
 * from `ret_leaves[0..ret_nleaves]`. Returns a handle for `thx_ffi_call_prep`
 * (free it with `thx_ffi_prep_free`), or NULL when libffi rejects the
 * signature. */
void *thx_ffi_prep(unsigned nargs,
                   const int *kinds,
                   const int *leaf_off, const int *leaf_len, const int *leaves,
                   int ret_kind, const int *ret_leaves, int ret_nleaves) {
    if (nargs > MAX_ARGS) return NULL;
    size_t naggs = ret_kind == K_STRUCT, nelems = ret_kind == K_STRUCT ? ret_nleaves + 1 : 0;
    for (unsigned i = 0; i < nargs; i++)
        if (kinds[i] == K_STRUCT) {
            if (leaf_len[i] > MAX_LEAVES) return NULL;
            naggs++;
            nelems += leaf_len[i] + 1;
        }
    if (ret_kind == K_STRUCT && ret_nleaves > MAX_LEAVES) return NULL;
    thx_prep_t *p = calloc(1, sizeof *p);
    if (!p) return NULL;
    p->atypes = calloc(nargs ? nargs : 1, sizeof *p->atypes);
    p->aggs = calloc(naggs ? naggs : 1, sizeof *p->aggs);
    p->elems = calloc(nelems ? nelems : 1, sizeof *p->elems);
    if (!p->atypes || !p->aggs || !p->elems) {
        thx_ffi_prep_free(p);
        return NULL;
    }
    ffi_type *agg = p->aggs;
    ffi_type **elems = p->elems;
    for (unsigned i = 0; i < nargs; i++) {
        if (kinds[i] == K_STRUCT) {
            build_struct(agg, elems, &leaves[leaf_off[i]], leaf_len[i]);
            p->atypes[i] = agg++;
            elems += leaf_len[i] + 1;
        } else {
            p->atypes[i] = ty(kinds[i]);
        }
    }
    ffi_type *rtype = ty(ret_kind);
    if (ret_kind == K_STRUCT) {
        build_struct(agg, elems, ret_leaves, ret_nleaves);
        rtype = agg;
    }
    if (ffi_prep_cif(&p->cif, FFI_DEFAULT_ABI, nargs, rtype,
                     nargs ? p->atypes : NULL) != FFI_OK) {
        thx_ffi_prep_free(p);
        return NULL;
    }
    return p;
}

/* Perform one call through a prepared interface. `avalues[i]` points at
 * argument i's value cell (a struct's bytes for a struct argument); `rvalue`
 * must have room for the result (>= struct size, or `sizeof(ffi_arg)`), 8-byte
 * aligned. A prepared cif is only read here, so a nested call (from a callback)
 * may reuse it. */
void thx_ffi_call_prep(void *handle, void *fn, void **avalues, void *rvalue) {
    thx_prep_t *p = handle;
    ffi_call(&p->cif, FFI_FN(fn), rvalue, p->cif.nargs ? avalues : NULL);
}

/* -- closures: a Thrax function passed to C as a function pointer -- */

/* The Rust dispatcher, invoked whenever foreign C calls the closure. It reads
 * the arguments (per the kinds captured at creation), applies the Thrax closure,
 * and writes the result. `user` is the Rust context pointer given to
 * `thx_closure_new`. */
extern void thx_closure_invoke(void *user, void **args, void *ret);

typedef struct {
    ffi_closure *closure;
    ffi_cif cif;
    ffi_type *atypes[MAX_ARGS];
    void *code;
    void *user;
} thx_closure_t;

static void thx_closure_binding(ffi_cif *cif, void *ret, void **args, void *user) {
    (void)cif;
    thx_closure_t *h = user;
    thx_closure_invoke(h->user, args, ret);
}

/* Allocate and prepare a closure over the scalar `kinds` (leaf codes; structs
 * are not supported in a callback signature). Writes the C-callable code pointer
 * to `*code_out` and returns an opaque handle to free with `thx_closure_free`.
 * `user` is passed back to `thx_closure_invoke`. Returns NULL on failure. */
void *thx_closure_new(unsigned nargs, const int *kinds, int ret_kind,
                      void *user, void **code_out) {
    if (nargs > MAX_ARGS) return NULL;
    thx_closure_t *h = calloc(1, sizeof(thx_closure_t));
    if (!h) return NULL;
    h->user = user;
    for (unsigned i = 0; i < nargs; i++) h->atypes[i] = ty(kinds[i]);
    if (ffi_prep_cif(&h->cif, FFI_DEFAULT_ABI, nargs, ty(ret_kind),
                     nargs ? h->atypes : NULL) != FFI_OK) {
        free(h);
        return NULL;
    }
    h->closure = ffi_closure_alloc(sizeof(ffi_closure), &h->code);
    if (!h->closure) {
        free(h);
        return NULL;
    }
    if (ffi_prep_closure_loc(h->closure, &h->cif, thx_closure_binding, h, h->code)
        != FFI_OK) {
        ffi_closure_free(h->closure);
        free(h);
        return NULL;
    }
    *code_out = h->code;
    return h;
}

void thx_closure_free(void *handle) {
    thx_closure_t *h = handle;
    if (h) {
        ffi_closure_free(h->closure);
        free(h);
    }
}
