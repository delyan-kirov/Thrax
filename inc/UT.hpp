#ifndef UT_HEADER
#define UT_HEADER

#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <initializer_list>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

#include "AR.hpp"

// TODO: There should be a special format for macro args
// This is because arguments can resolve to other macros
// Which is dangerous and difficult to debug
// Therefore, arguments should follow some type of convention
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

// TODO: should report line and file
#define UT_TODO(TODO_MSG)                                                      \
  UT::IMPL::fail_if(                                                           \
    __FILE__, __PRETTY_FUNCTION__, __LINE__, UT::STODO, #TODO_MSG)

#define UT_FAIL_IF(CONDITION)                                                  \
  do                                                                           \
  {                                                                            \
    if (CONDITION)                                                             \
    {                                                                          \
      UT::IMPL::fail_if(                                                       \
        __FILE__, __PRETTY_FUNCTION__, __LINE__, UT::SERROR, #CONDITION);      \
    }                                                                          \
  } while (false)

#define UT_FAIL_MSG(MSG_FORMAT, ...)                                           \
  do                                                                           \
  {                                                                            \
    char *UT_STRING_BUFFER_NO_ESCAPE = nullptr;                                \
    asprintf(&UT_STRING_BUFFER_NO_ESCAPE, MSG_FORMAT, __VA_ARGS__);            \
    UT::IMPL::fail_if(__FILE__,                                                \
                      __PRETTY_FUNCTION__,                                     \
                      __LINE__,                                                \
                      UT::SERROR,                                              \
                      UT_STRING_BUFFER_NO_ESCAPE);                             \
  } while (false)

#define UT_TCS(o) (std::to_string(o).c_str())

#define UT_VAR_INSP(UT_VAR)                                                    \
  do                                                                           \
  {                                                                            \
    UT_FAIL_MSG("INFO (%s)", UT_TCS(UT_VAR));                                  \
  } while (false)

#define ARRAY_LEN(UT_ARRAY_OBJ)                                                \
  (sizeof(UT_ARRAY_OBJ) / (sizeof(UT_ARRAY_OBJ[0])))

#define UT_WARNING(MSG_FORMAT, ...)                                            \
  do                                                                           \
  {                                                                            \
    std::fprintf(stderr, "WARN: ");                                            \
    std::fprintf(stderr, MSG_FORMAT, __VA_ARGS__);                             \
    std::fprintf(stderr, "\n");                                                \
  } while (false)

#define UT_UNUSED(UT_UNUSED_VAR) (void)UT_UNUSED_VAR

#define UTSTRf "%.*s"
#define UTSTFa(UT_STR_VAR) (int)UT_STR_VAR.m_len, UT_STR_VAR.m_mem

namespace UT
{

constexpr const char *STODO  = "TODO";
constexpr const char *SERROR = "\033[31mERROR\033[0m";

namespace IMPL
{

inline void
abort()
{
#if defined(_MSC_VER)
  std::abort();
#elif defined(__x86_64__) || defined(_M_X64) || defined(__i386__)              \
  || defined(_M_IX86)
  asm("int3");
#else
  std::abort();
#endif
};

inline void
fail_if(
  const char *file,    //
  const char *fn_name, //
  const int   line,    //
  const char *prefix,  //
  const char *msg)
{
  if (msg)
  {
    std::printf("[%s] %s : %s\n", prefix, file, fn_name);
    std::printf("  %d | \033[1;37m%s\033[0m\n", line, msg);
    UT::IMPL::abort();
  }
}

} // namespace IMPL

constexpr size_t V_DEFAULT_MAX_LEN = 1 << 6;

template <typename O> struct Vu
{
  O     *m_mem;
  size_t m_len;

  Vu()
      : m_mem{ nullptr },
        m_len{ 0 } {};
  constexpr Vu(O *o, size_t len)
      : m_mem{ o },
        m_len{ len } {};
  constexpr Vu(const char *s, size_t len)
      : m_mem{ (char *)s },
        m_len{ len } {};

  Vu(
    const char *s)
      : m_mem{ s }
  {
    if (s)
    {
      this->m_len = std::strlen(s);
    }
    else
    {
      UT_FAIL_IF("Provided string is null");
    }
  };

  Vu(
    std::string &s)
      : m_mem{ s.c_str() },
        m_len(s.size())
  {
  }

  template <typename T>
  Vu(
    std::vector<T> &v)
      : m_mem{ v.data() },
        m_len{ v.size() }
  {
  }

  bool
  is_empty()
  {
    return 0 == m_len;
  };

  O *
  get()
  {
    return this->m_mem;
  };

  O &
  operator[](
    size_t i)
  { // for writing
    return this->m_mem[i];
  };

  const O &
  operator[](
    size_t i) const
  { // for reading from const objects
    return this->m_mem[i];
  };

  const O *
  begin() const
  {
    return this->m_mem;
  };
  const O *
  end() const
  {
    return this->m_mem + this->m_len;
  };
  O *
  begin()
  {
    return this->m_mem;
  };
  O *
  end()
  {
    return this->m_mem + this->m_len;
  };

  O *
  first()
  {
    return this->m_mem;
  };

  O *
  last()
  {
    return this->m_mem + (this->m_len - 1);
  };

  O *
  pop_front()
  {
    O *elem = m_mem;
    m_mem += 1;
    m_len -= 1;
    return elem;
  }

  O *
  pop_back()
  {
    O *elem = m_mem[m_len - 1];
    m_len -= 1;
    return elem;
  }

  void
  retreat()
  {
    m_mem -= 1;
    m_len += 1;
  }

  O *
  forsakened(
    size_t i = 1)
  {
    return m_mem - i;
  }
};

struct String : public Vu<char>
{
  template <size_t N>
  constexpr String(
    const char (&mem)[N])
      : Vu<char>{ mem, N - 1 }
  {
  }
  // Construct from pointer + length
  String(
    const char *mem, size_t len)
      : Vu<char>{ (char *)mem, len }
  {
  }
  String(
    char *mem, size_t len)
      : Vu<char>{ mem, len }
  {
  }

  String(
    char *s)
  {
    this->m_len = std::strlen(s);
    this->m_mem = s;
  }

  String(
    int i)
  {
    void *mem = (void *)this;
    std::memset(mem, i, sizeof(String));
  }

  String()                          = default;
  String(const String &)            = default;
  String(String &&)                 = default;
  String &operator=(const String &) = default;
  String &operator=(String &&)      = default;

  const char *
  to_cstr(
    AR::Arena &arena)
  {
    char *mem = (char *)arena.alloc((this->m_len + 1) * sizeof(char));
    std::memset(mem, 0, this->m_len + 1);
    std::strcpy(mem, this->m_mem);
    return mem;
  }

  bool
  operator==(
    String &other) const
  {
    return (this->m_len == other.m_len)
           && (0 == std::memcmp(this->m_mem, other.m_mem, this->m_len));
  }

  bool
  operator!=(
    String &other) const
  {
    return !((this->m_len == other.m_len)
             && (0 == std::memcmp(this->m_mem, other.m_mem, this->m_len)));
  }

  template <typename T>
  using is_non_char_integral
    = std::integral_constant<bool,
                             std::is_integral<T>::value
                               && !std::is_same<T, char>::value
                               && !std::is_same<T, signed char>::value
                               && !std::is_same<T, unsigned char>::value>;

  template <typename T,
            typename
            = typename std::enable_if<is_non_char_integral<T>::value>::type>
  void
  operator+=(
    T i)
  {
    this->m_len += i;
  }

  template <typename T,
            typename
            = typename std::enable_if<is_non_char_integral<T>::value>::type>
  void
  operator-=(
    T i)
  {
    this->m_len -= i;
  }
};

inline bool
operator==(
  const char *left, UT::String right)
{
  size_t other_len = std::strlen(left);
  return (right.m_len == other_len)
         && (0 == std::memcmp(right.m_mem, left, right.m_len));
}

inline bool
operator==(
  UT::String right, const char *left)
{
  size_t other_len = std::strlen(left);
  return (right.m_len == other_len)
         && (0 == std::memcmp(right.m_mem, left, right.m_len));
}

inline bool
operator!=(
  const char *left, UT::String right)
{
  size_t other_len = std::strlen(left);
  return !((right.m_len == other_len)
           && (0 == std::memcmp(right.m_mem, left, right.m_len)));
}

inline bool
operator!=(
  UT::String right, const char *left)
{
  size_t other_len = std::strlen(left);
  return !((right.m_len == other_len)
           && (0 == std::memcmp(right.m_mem, left, right.m_len)));
}

inline String
memcopy(
  AR::Arena &arena, const char *s)
{
  size_t s_len = std::strlen(s);
  auto   new_s = (char *)arena.alloc(s_len + 1);
  (void)std::memcpy(new_s, s, s_len);
  String result{ new_s, s_len };
  return result;
}

inline String
strdup(
  AR::Arena &arena, const char *s, size_t len)
{
  auto new_s = (char *)arena.alloc(len);
  (void)std::memcpy(new_s, s, len);
  String result{ new_s, len };
  return result;
}

inline String
strdup(
  AR::Arena &arena, const char *s)
{
  size_t len   = std::strlen(s);
  auto   new_s = (char *)arena.alloc(len + 1);
  (void)std::memcpy(new_s, s, len);
  new_s[len] = 0;
  String result{ new_s, len };
  return result;
}

inline bool
strcompare(
  const String s1, const String s2)
{
  return s1.m_len == s2.m_len && 0 == std::memcmp(s1.m_mem, s2.m_mem, s1.m_len);
}

inline String
strdup(
  AR::Arena &arena, String s)
{
  auto new_s = (char *)arena.alloc(s.m_len + 1);
  (void)std::memcpy(new_s, s.m_mem, s.m_len);
  new_s[s.m_len] = 0;
  String result{ new_s, s.m_len };
  return result;
}

template <typename O> struct Vec
{
  O         *m_mem;
  size_t     m_len;
  size_t     m_max_len;
  AR::Arena *m_arena;

  Vec()                       = default;
  ~Vec()                      = default;
  Vec(const Vec &other)       = default;
  Vec &operator=(const Vec &) = default;

  Vec(
    Vec &&other)
      : Vec{ other }
  {
    other.m_arena   = nullptr;
    other.m_len     = 0;
    other.m_max_len = 0;
    other.m_mem     = nullptr;
  }

  Vec(
    AR::Arena &arena, size_t len = 0)
      : m_len{ 0 },
        m_max_len{ V_DEFAULT_MAX_LEN },
        m_arena{ &arena }
  {
    size_t alloc_len = (0 == len) ? V_DEFAULT_MAX_LEN : len;
    this->m_mem      = (O *)arena.alloc<O>(alloc_len);
  };

  Vec(
    std::initializer_list<size_t> lst)
      : m_arena{ 0 },
        m_len{ 0 },
        m_max_len{ 0 },
        m_mem{ 0 }
  {
    (void)lst;
  };

  Vec(
    AR::Arena &arena, std::initializer_list<O> lst)
      : m_arena{ 0 },
        m_len{ 0 },
        m_max_len{ V_DEFAULT_MAX_LEN },
        m_mem{ 0 }
  {
    this->m_mem = (O *)arena.alloc<O>(this->m_max_len);
    for (const O &o : lst)
    {
      this->push(o);
    }
  };

  const O *
  begin() const
  {
    return this->m_mem;
  };
  const O *
  end() const
  {
    return this->m_mem + this->m_len;
  };
  O *
  begin()
  {
    return this->m_mem;
  };
  O *
  end()
  {
    return this->m_mem + this->m_len;
  };

  O *
  last()
  {
    return this->m_mem + (this->m_len - 1);
  };

  O &
  operator[](
    size_t i)
  { // for writing
    return this->m_mem[i];
  };

  const O &
  operator[](
    size_t i) const
  { // for reading from const objects
    return this->m_mem[i];
  };

  void
  push(
    O o)
  {
    if (this->m_len >= this->m_max_len)
    {
      // We need more space
      O *new_mem = (O *)this->m_arena->alloc(sizeof(O) * 2 * this->m_max_len);
      std::memcpy((void *)new_mem, this->m_mem, sizeof(O) * this->m_len);

      this->m_mem = new_mem;
      this->m_max_len *= 2;
    }
    this->m_mem[this->m_len] = o;
    this->m_len += 1;
  };

  O
  pop()
  {
    O o = *this->last();
    m_len -= 1;
    return o;
  };

  bool
  is_empty()
  {
    return 0 == this->m_len;
  }
};

template <typename T> class Pair
{
  T *data;

public:
  Pair(
    AR::Arena &arena)
  {
    this->data = (T *)arena.alloc<T>(2);
  };
  Pair() = default;
  T
  first()
  {
    return data[0];
  }
  T
  second()
  {
    return data[1];
  }
  T *
  begin()
  {
    return data;
  }
  T *
  end()
  {
    return data + 2;
  }
  T *
  last()
  {
    return data + 1;
  }
};

class SB
{
public:
  char  *m_mem;
  size_t m_len;
  size_t m_max_len;

  SB()
      : m_len{ 0 }
  {
    this->m_mem     = new char[sizeof(char) * V_DEFAULT_MAX_LEN];
    this->m_max_len = V_DEFAULT_MAX_LEN;
    std::memset(this->m_mem, 0, this->m_max_len);
  }

  SB(const SB &)            = delete; // copy constructor
  SB &operator=(const SB &) = delete; // copy assignment
  SB(SB &&)                 = delete; // move constructor
  SB &operator=(SB &&)      = delete; // move assignment

  ~SB()
  {
    if (this->m_mem)
    {
      delete[] this->m_mem;
      this->m_mem = nullptr;
    }
  }

  void
  resize(
    size_t new_len)
  {
    size_t new_max_len = 2 * (this->m_max_len + new_len);
    char  *new_mem     = new char[new_max_len];
    std::memset(new_mem, 0, new_max_len);
    std::strcpy(new_mem, this->m_mem);
    delete[] this->m_mem;
    this->m_mem     = new_mem;
    this->m_max_len = new_max_len;
  }

  void
  add(
    const char *s)
  {
    size_t available_space = this->m_max_len - this->m_len;
    size_t s_len           = std::strlen(s);
    if (available_space < s_len)
    {
      this->resize(s_len);
    }
    std::strcat(this->m_mem, s);
    this->m_len += s_len;
  }

  void
  add(
    const char c)
  {
    size_t available_space = this->m_max_len - this->m_len;
    size_t s_len           = 1;
    if (available_space < s_len)
    {
      this->resize(s_len);
    }
    this->m_mem[this->m_len] = c;
    this->m_len += s_len;
  }

  template <typename... Args> void concat(Args &&...args);
  template <typename... Args> void concatf(const char *fmt, Args &&...args);
  template <typename... Args> void append(Args &&...args);

  const String
  to_String(
    AR::Arena &arena)
  {
    char *mem = (char *)arena.alloc(sizeof(char) * this->m_len + 1);
    std::memset(mem, 0, this->m_len + 1);
    std::memcpy(mem, this->m_mem, this->m_len);
    return String{ mem, this->m_len };
  }

  const char *
  to_cstr(
    AR::Arena &arena)
  {
    char *mem = (char *)arena.alloc(sizeof(char) * this->m_len + 1);
    std::memset(mem, 0, this->m_len + 1);
    std::memcpy(mem, this->m_mem, this->m_len);
    return mem;
  }

  const String
  vu()
  {
    return String{ this->m_mem, this->m_len };
  }

  SB &
  operator>>(
    const char *s)
  {
    this->add(s);
    return *this;
  }

  SB &
  operator>>(
    String str)
  {
    this->add(str.m_mem);
    return *this;
  }

  SB &
  operator>>(
    SB &sb)
  {
    String vu = sb.vu();
    this->add(vu.m_mem);
    return *this;
  }

  template <typename T>
  SB &
  operator>>(
    T &t)
  {
    std::string s = std::to_string(t);
    this->add(s.c_str());
    return *this;
  }
};

template <typename... Args>
void
SB::concatf(
  const char *fmt, Args &&...args)
{
  char *buffer;
  (void)asprintf(&buffer, fmt, std::forward<Args>(args)...);
  this->concat(buffer);
  std::free(buffer);
}

template <typename... Args>
void
SB::concat(
  Args &&...args)
{
  (..., this->add(std::forward<Args>(args)));
}

template <typename... Args>
void
SB::append(
  Args &&...args)
{
  (..., this->concat(std::forward<Args>(args), " "));
}

// TODO: Better print messages
// TODO: Better error handling
inline String
read_entire_file(
  UT::String file_name, AR::Arena &arena)
{
  const char *file_str = file_name.m_mem;
  size_t      file_len = 0;
  char       *buffer   = nullptr;
  size_t      result   = 0;

  FILE *file_stream = std::fopen(file_str, "rb");
  if (!file_stream)
  {
    std::fprintf(stderr, "ERROR: could not open file: %s\n", file_str);
    goto DEFER_RETURN;
  }

  std::fseek(file_stream, 0, SEEK_END);
  file_len = ftell(file_stream);

  std::rewind(file_stream);
  buffer           = (char *)arena.alloc(sizeof(char) * (file_len + 1));
  buffer[file_len] = 0;

  result = std::fread(buffer, 1, file_len, file_stream);
  if (result != file_len)
  {
    std::fprintf(
      stderr, "ERROR: could not map file %s to memory buffer\n", file_str);

    goto DEFER_RETURN;
  }

DEFER_RETURN:
  std::fclose(file_stream);
  return UT::String{ buffer, file_len };
}

} // namespace UT

namespace std
{
inline string
to_string(
  UT::String s)
{
  const char *var_mem = new char[s.m_len + 1];
  memset((void *)var_mem, 0, s.m_len + 1);
  memcpy((void *)var_mem, s.begin(), s.m_len);
  string result{ var_mem };
  delete[] var_mem;
  return result;
}
} // namespace std

#endif // UT_HEADER
