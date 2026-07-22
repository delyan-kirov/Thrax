/*-------------------------------------------------------------------------------
 *\file THxPLAT.h
 *\info The runtime's platform detection, centralized. This is the ONLY file
 *      in the runtime that may consult predefined platform macros (and,
 *      with compiler/TG.hpp's host() plus the exempted build-tool shims, one
 *      of the few places in the project allowed to -- `build check-platform`
 *      enforces the boundary by mechanism). Everything else keys off the
 *      THX_OS_* / THX_PTR_BITS macros and the THX_INT_T type below.
 *
 *      The runtime is baked into every generated program as text and
 *      compiled by the TARGET's C compiler, so detection here is TARGET
 *      truth -- and the `_Static_assert(sizeof(void*))` domino CC emits
 *      guarantees it agrees with what the Thrax compiler chose.
 *-----------------------------------------------------------------------------*/

#ifndef THX_PLAT_H_
#define THX_PLAT_H_

/* Operating system: exactly one THX_OS_* is 1, the rest 0. */
#if defined(_WIN32)
#define THX_OS_WINDOWS 1
#elif defined(__wasi__)
#define THX_OS_WASI 1
#elif defined(__APPLE__)
#define THX_OS_MACOS 1
#else
#define THX_OS_LINUX 1
#endif

#ifndef THX_OS_WINDOWS
#define THX_OS_WINDOWS 0
#endif
#ifndef THX_OS_WASI
#define THX_OS_WASI 0
#endif
#ifndef THX_OS_MACOS
#define THX_OS_MACOS 0
#endif
#ifndef THX_OS_LINUX
#define THX_OS_LINUX 0
#endif

/* Pointer width in bits: the target word, and therefore the width of the
 * language's `Int` (TG::Target::int_ty makes the same choice in the
 * compiler; the domino assert keeps the two honest). */
#if defined(__SIZEOF_POINTER__)
#define THX_PTR_BITS (__SIZEOF_POINTER__ * 8)
#elif defined(_WIN64)
#define THX_PTR_BITS 64
#elif defined(_WIN32)
#define THX_PTR_BITS 32
#else
#error "THxPLAT: cannot determine the target pointer width"
#endif

/* The runtime representation of the language's `Int`: the target word. A
 * 32-bit target stores (and therefore wraps) integers at 32 bits, so the
 * runtime semantics match the type. APIs still traffic in `long long` (a
 * superset on every target); the truncation on store is the wrap. */
#include <stdint.h>
#if THX_PTR_BITS == 64
typedef int64_t THX_INT_T;
#else
typedef int32_t THX_INT_T;
#endif

#endif /* THX_PLAT_H_ */
