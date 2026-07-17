#ifndef UT_HEADER_
#define UT_HEADER_

#include <algorithm>
#include <cmath>
#include <cstdarg>
#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <filesystem>
#include <fstream>
#include <functional>
#include <initializer_list>
#include <memory.h>
#include <memory>
#include <string>
#include <string_view>
#include <system_error>
#include <type_traits>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <variant>
#include <vector>

#include "AR.hpp"

#if defined(__GNUC__) || defined(__clang__)
#define UT_PRINTF_LIKE(fmt_idx, arg_idx)                                       \
  __attribute__((format(printf, fmt_idx, arg_idx)))
#else
#define UT_PRINTF_LIKE(fmt_idx, arg_idx)
#endif

#if __cplusplus >= 201703L
#define UT_NODISCARD [[nodiscard]]
#elif defined(__GNUC__) || defined(__clang__)
#define UT_NODISCARD __attribute__((warn_unused_result))
#elif defined(_MSC_VER)
#define UT_NODISCARD _Check_return_
#else
#define UT_NODISCARD
#endif

#define UT_TODO(MSG)                                                           \
  UT::fail_if(__FILE__, __PRETTY_FUNCTION__, __LINE__, "TODO", #MSG)

#define UT_FAIL_IF(COND)                                                       \
  do                                                                           \
  {                                                                            \
    if (COND)                                                                  \
    {                                                                          \
      UT::fail_if(__FILE__,                                                    \
                  __PRETTY_FUNCTION__,                                         \
                  __LINE__,                                                    \
                  "\033[31mERROR\033[0m",                                      \
                  #COND);                                                      \
    }                                                                          \
  } while (false)

#define UT_FAIL_MSG(FMT, ...)                                                  \
  do                                                                           \
  {                                                                            \
    char *buf_ = nullptr;                                                      \
    asprintf(&buf_, FMT, __VA_ARGS__);                                         \
    UT::fail_if(                                                               \
      __FILE__, __PRETTY_FUNCTION__, __LINE__, "\033[31mERROR\033[0m", buf_);  \
  } while (false)

#define ARRAY_LEN(ARR) (sizeof(ARR) / (sizeof(ARR[0])))

#define UT_UNUSED(VAR) (void)VAR

namespace UT
{

void abort();

void fail_if(const char *file,    //
             const char *fn_name, //
             const int   line,    //
             const char *prefix,  //
             const char *msg);

constexpr size_t V_DEFAULT_MAX_LEN = 1 << 6;

// A non-owning view over a run of chars -- a slice of arena (or source) memory.
// This is exactly std::string_view; the alias keeps the project-local spelling
// and gives the arena helpers below something to hang off of.
using Vu = std::string_view;

// Copy bytes into the arena and return a view that outlives the source. The
// copy is null-terminated, so the result can be handed to C string APIs.
Vu strdup(AR::Arena &arena, Vu s);
Vu strdup(AR::Arena &arena, const char *s);

template <typename O> struct Vec
{
  O         *mem;
  size_t     len;
  size_t     max_len;
  AR::Arena *arena;

  Vec()                       = default;
  ~Vec()                      = default;
  Vec(const Vec &other)       = default;
  Vec &operator=(const Vec &) = default;

  Vec(
    Vec &&other)
      : Vec{ other }
  {
    other.arena   = nullptr;
    other.len     = 0;
    other.max_len = 0;
    other.mem     = nullptr;
  }

  Vec(
    AR::Arena &arena, size_t len = 0)
      : len{ 0 },
        max_len{ V_DEFAULT_MAX_LEN },
        arena{ &arena }
  {
    size_t alloc_len = (0 == len) ? V_DEFAULT_MAX_LEN : len;
    this->mem        = (O *)arena.alloc<O>(alloc_len);
  };

  Vec(
    std::initializer_list<size_t> lst)
      : arena{ 0 },
        len{ 0 },
        max_len{ 0 },
        mem{ 0 }
  {
    UT_UNUSED(lst);
  };

  Vec(
    AR::Arena &arena, std::initializer_list<O> lst)
      : arena{ 0 },
        len{ 0 },
        max_len{ V_DEFAULT_MAX_LEN },
        mem{ 0 }
  {
    this->mem = (O *)arena.alloc<O>(this->max_len);
    for (const O &o : lst)
    {
      this->push(o);
    }
  };

  const O *
  begin() const
  {
    return mem;
  };
  const O *
  end() const
  {
    return mem + len;
  };
  O *
  begin()
  {
    return mem;
  };
  O *
  end()
  {
    return mem + len;
  };

  size_t
  size() const
  {
    return len;
  };
  O *
  data()
  {
    return mem;
  };
  const O *
  data() const
  {
    return mem;
  };

  O *
  last()
  {
    return mem + (len - 1);
  };

  O &
  operator[](
    size_t i)
  { // for writing
    return mem[i];
  };

  const O &
  operator[](
    size_t i) const
  { // for reading from const objects
    return mem[i];
  };

  void
  push(
    O o)
  {
    if (len >= max_len)
    {
      // We need more space
      O *new_mem = (O *)arena->alloc(sizeof(O) * 2 * max_len);
      std::memcpy((void *)new_mem, mem, sizeof(O) * len);

      mem = new_mem;
      max_len *= 2;
    }
    mem[len] = o;
    len += 1;
  };

  O
  pop()
  {
    O o = *last();
    len -= 1;
    return o;
  };

  bool
  empty()
  {
    return 0 == len;
  }
};

// Look up a key in any map-like container, returning a pointer to the mapped
// value, or nullptr when absent. Use when not finding the key is a normal case.
template <typename Map, typename Key>
const typename Map::mapped_type *
try_lookup(
  const Map &m, const Key &k)
{
  auto it = m.find(k);
  return it == m.end() ? nullptr : &it->second;
}

// Look up a key that is expected to be present; fails hard when it is not.
template <typename Map, typename Key>
const typename Map::mapped_type &
lookup(
  const Map &m, const Key &k)
{
  auto it = m.find(k);
  UT_FAIL_IF(it == m.end());
  return it->second;
}

// Look up a key, falling back to a default value when it is absent.
template <typename Map, typename Key>
typename Map::mapped_type
lookup_or(
  const Map &m, const Key &k, typename Map::mapped_type fallback)
{
  auto it = m.find(k);
  return it == m.end() ? fallback : it->second;
}

} // namespace UT

#endif // UT_HEADER_
