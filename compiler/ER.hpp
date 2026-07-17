/*-------------------------------------------------------------------------------
 *\file ER.hpp
 *\info Shared error/diagnostic facility for the lexer (LX) and parser (EX).
 *
 * The model is a small value type, ER::Result<T>, that holds either a value or
 * an ER::Diagnostic. A Diagnostic is a chain of ER::Frame, root cause first,
 * each anchored to a view into the source so we can render a caret. Context is
 * added on the way up by the caller (see the CTX macro in EX/LX), so a frame
 * exists only if the error actually propagated through it -- there is no shared
 * error buffer to leak on the success path.
 *-----------------------------------------------------------------------------*/

#ifndef ER_HEADER_
#define ER_HEADER_

#include "AR.hpp"
#include "UT.hpp"

namespace ER
{

/*------------------------------------------------------------------------------
 *\ERROR CODES
 *-----------------------------------------------------------------------------*/

#define ER_CODE_VARIANTS                                                       \
  X(OK)                                                                        \
  X(ASCII_CTR_CHAR)                                                            \
  X(NON_ASCII_CHAR)                                                            \
  X(ILLEGAL_RESERVED_CHAR)                                                     \
  X(QUOTM_UNCLOSED)                                                            \
  X(INVALID_ESCAPE)                                                            \
  X(INVALID_UTF8)                                                              \
  X(NUMBER_PARSING_FAILURE)                                                    \
  X(UNKNOWN_SYMBOL)                                                            \
  X(EXPECTED_OPERAND)                                                          \
  X(UNEXPECTED_TOKEN)                                                          \
  X(PARENTHESIS_UNBALANCED)                                                    \
  X(EXPECTED_GLOBAL)                                                           \
  X(UNSUPPORTED)                                                               \
  X(TYPE_MISMATCH)                                                             \
  X(TYPE_UNBOUND)                                                              \
  X(TYPE_ANNOTATION_REQUIRED)                                                  \
  X(TYPE_CYCLE)                                                                \
  X(BAD_MODULE_NAME)                                                           \
  X(FILENAME_MISMATCH)                                                         \
  X(AMBIGUOUS_NAME)                                                            \
  X(DUPLICATE_SYMBOL)                                                          \
  X(UNKNOWN_MODULE)                                                            \
  X(PRIVATE_SYMBOL)                                                            \
  X(NO_ENTRY)                                                                  \
  X(ENTRY_SIGNATURE)                                                           \
  X(ASSERT_FAILED)

enum class Code
{
#define X(c) c,
  ER_CODE_VARIANTS
#undef X
};

inline std::string
pprint(
  Code c)
{
  switch (c)
  {
#define X(c)                                                                   \
  case Code::c: return #c;
    ER_CODE_VARIANTS
#undef X
  }
  return "UNKNOWN";
}

/*------------------------------------------------------------------------------
 *\DIAGNOSTIC TYPES
 *-----------------------------------------------------------------------------*/

// One link in an error's context chain. `anchor` is a view into the source
// (used to draw the caret); `line` is its 1-based source line.
struct Frame
{
  Code   code;
  UT::Vu anchor;
  size_t line;
  UT::Vu msg;
  Frame *next; // toward the outer context
};

// A diagnostic is a chain of frames, root cause (`root`) first.
struct Diagnostic
{
  Frame *root = nullptr;
  Frame *tail = nullptr;
};

template <typename T> struct Result
{
  bool       ok = false;
  T          value{};
  Diagnostic err{};
};

// Proxy so a bare error can convert into Result<T> for whatever T the enclosing
// function returns -- this is what lets TRY/CTX stay agnostic of T.
struct Fail
{
  Diagnostic err;
  template <typename T>
  operator Result<T>() const
  {
    return Result<T>{ false, T{}, err };
  }
};

/*------------------------------------------------------------------------------
 *\CONSTRUCTION HELPERS
 *-----------------------------------------------------------------------------*/

// Format a printf-style message into the arena so it outlives the call.
UT_PRINTF_LIKE(
  2, 3)
inline UT::Vu
mk_msg(
  AR::Arena &arena, const char *fmt, ...)
{
  va_list args;
  va_start(args, fmt);
  va_list copy;
  va_copy(copy, args);

  int len = std::vsnprintf(nullptr, 0, fmt, args);
  va_end(args);
  if (len < 0)
  {
    va_end(copy);
    return UT::Vu{ "<message formatting failed>" };
  }

  char *mem = (char *)arena.alloc((size_t)len + 1);
  std::vsnprintf(mem, (size_t)len + 1, fmt, copy);
  va_end(copy);
  return UT::Vu{ mem, (size_t)len };
}

inline Diagnostic
mk_root(
  AR::Arena &arena, Code code, UT::Vu anchor, size_t line, UT::Vu msg)
{
  Frame *f = (Frame *)arena.alloc<Frame>(1);
  *f       = Frame{ code, anchor, line, msg, nullptr };
  return Diagnostic{ f, f };
}

inline void
push_ctx(
  AR::Arena &arena, Diagnostic &d, UT::Vu anchor, size_t line, UT::Vu msg)
{
  Frame *f = (Frame *)arena.alloc<Frame>(1);
  *f = Frame{ d.root ? d.root->code : Code::OK, anchor, line, msg, nullptr };
  if (d.tail)
    d.tail->next = f;
  else
    d.root = f;
  d.tail = f;
}

/*------------------------------------------------------------------------------
 *\RENDERING
 *-----------------------------------------------------------------------------*/

// Render a diagnostic against the source it points into. Each frame is printed
// root-cause first, then the contexts that wrap it, with the caret anchored at
// the frame's own token.
inline std::string
pprint(
  const Diagnostic &d, UT::Vu input, UT::Vu filename)
{
  std::string s;
  size_t      depth = 0;

  for (Frame *f = d.root; f; f = f->next)
  {
    std::string pad(depth * 2, ' ');

    bool in_src = f->anchor.data() && input.data()
                  && f->anchor.data() >= input.data()
                  && f->anchor.data() < input.data() + input.size();
    size_t off = in_src ? (size_t)(f->anchor.data() - input.data()) : 0;
    size_t ls  = off;
    while (ls > 0 && input.data()[ls - 1] != '\n') ls -= 1;
    size_t le = off;
    while (le < input.size() && input.data()[le] != '\n') le += 1;

    std::string line_txt(input.data() + ls, le - ls);
    size_t      col = off - ls;
    // Compute the line number from the offset so it is correct even when the
    // anchor points into a different source file than `f->line` was set for
    // (multi-file builds reuse one diagnostic type across units).
    size_t lineno = 1;
    for (size_t i = 0; in_src && i < off; ++i)
      if (input.data()[i] == '\n') lineno += 1;
    std::string ln = in_src ? std::to_string(lineno) : std::to_string(f->line);

    s += pad + "\033[31m[" + pprint(f->code) + "]\033[0m "
         + std::string(filename) + ":" + ln + "\n";
    s += pad + "  " + ln + " | \033[1;37m" + line_txt + "\033[0m\n";
    s += pad + "  " + std::string(ln.size(), ' ') + " | "
         + std::string(col, ' ') + "\033[31m^\033[0m\n";
    s += pad + "    " + std::string(f->msg) + "\n";

    depth += 1;
  }

  return s;
}

} // namespace ER

#undef ER_CODE_VARIANTS
#undef X

#endif // ER_HEADER_
