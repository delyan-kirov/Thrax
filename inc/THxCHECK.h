/*-------------------------------------------------------------------------------
 *\file THxCHECK.h
 *\info The C-runtime analog of the compiler's UT_FAIL family. Every invariant
 *      the generated code or the runtime relies on is asserted through these,
 *      so a violation aborts with a `file:line (fn): message` report and a
 *      non-zero exit -- never silent undefined behaviour. This is the backbone
 *      of the "exploreable" design: a mistaken edit trips a THxCHECK_ASSERT with a
 *      meaningful message rather than corrupting memory quietly.
 *-----------------------------------------------------------------------------*/

#ifndef THxCHECK_H_
#define THxCHECK_H_

/* Abort with a fixed message. */
#define THxCHECK_FAIL(MSG) THxCHECK_fail(__FILE__, __LINE__, __func__, (MSG))

/* Abort with a printf-style formatted message. */
#define THxCHECK_FAILF(...) THxCHECK_failf(__FILE__, __LINE__, __func__, __VA_ARGS__)

/* Assert `COND`; abort with `MSG` if it does not hold. */
#define THxCHECK_ASSERT(COND, MSG)                                                   \
  do                                                                           \
  {                                                                            \
    if (!(COND)) THxCHECK_fail(__FILE__, __LINE__, __func__, (MSG));                \
  } while (0)

#if defined(__GNUC__) || defined(__clang__)
#define THxCHECK_NORETURN __attribute__((noreturn))
#define THxCHECK_PRINTF_LIKE(fmt_idx, arg_idx)                                      \
  __attribute__((format(printf, fmt_idx, arg_idx)))
#else
#define THxCHECK_NORETURN
#define THxCHECK_PRINTF_LIKE(fmt_idx, arg_idx)
#endif

THxCHECK_NORETURN void
THxCHECK_fail(const char *file, int line, const char *fn, const char *msg);

THxCHECK_NORETURN THxCHECK_PRINTF_LIKE(4, 5) void THxCHECK_failf(
  const char *file, int line, const char *fn, const char *fmt, ...);

#endif /* THxCHECK_H_ */
