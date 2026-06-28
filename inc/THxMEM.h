/*-------------------------------------------------------------------------------
 *\file THxMEM.h
 *\info The memory / value-lifetime seam. The runtime and generated code touch
 *      allocation ONLY through this interface, so the strategy is swappable:
 *      v1 links ext/THxMEMBUMP.c (a bump arena that never frees and whose
 *      retain/release are no-ops). A future ext/THxMEMRC.c implements
 *      retain/release over Value::rc for reference counting -- at which point
 *      the codegen begins emitting THxMEM_retain/THxMEM_release at the documented
 *      binding/return/overwrite points (see doc/native-backend.md).
 *-----------------------------------------------------------------------------*/

#ifndef THxMEM_H_
#define THxMEM_H_

#include "THxVALUE.h"

#include <stddef.h>

/* Allocate `size` zeroed bytes with value lifetime. Never returns NULL (a
 * failed allocation aborts via THxCHECK_FAIL). */
void *THxMEM_alloc(size_t size);

/* Allocate one zeroed Value. */
Value *THxMEM_alloc_value(void);

/* Lifetime hooks. No-ops under the bump allocator; the ref-counting engine
 * makes them adjust Value::rc and free at zero. Both tolerate NULL. */
void THxMEM_retain(Value *v);
void THxMEM_release(Value *v);

#endif /* THxMEM_H_ */
