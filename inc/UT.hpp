#ifndef UT_HEADER
#define UT_HEADER

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <initializer_list>
#include <string>
#include <utility>

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
    char *s = nullptr;                                                         \
    asprintf(&s, MSG_FORMAT, __VA_ARGS__);                                     \
    UT::IMPL::fail_if(__FILE__, __PRETTY_FUNCTION__, __LINE__, UT::SERROR, s); \
  } while (false)

#define UT_TCS(o) (std::to_string(o).c_str())

#define UT_VAR_INSP(UT_VAR)                                                    \
  do                                                                           \
  {                                                                            \
    UT_FAIL_MSG("INFO (%s)", UT_TCS(UT_VAR));                                  \
  } while (false)

#define ARRAY_LEN(UT_ARRAY_OBJ)                                                \
  (sizeof(UT_ARRAY_OBJ) / (sizeof(UT_ARRAY_OBJ[0])))

namespace AR
{
constexpr size_t BLOCK_DEFAULT_LEN = (1 << 10);

class Arena;
class Block
{
  friend class Arena;

private:
  size_t  len;
  size_t  max_len;
  uint8_t mem[];

  Block()  = delete;
  ~Block() = delete;

  Block(const Block &)            = delete;
  Block &operator=(const Block &) = delete;

  Block(Block &&)            = delete;
  Block &operator=(Block &&) = delete;
};

class Arena
{
public:
  void *alloc(size_t size);

  template <typename Type>
  void *
  alloc()
  {
    return alloc(sizeof(Type));
  }

  template <typename Type>
  void *
  alloc(
    size_t size)
  {
    return alloc(size * sizeof(Type));
  }

  template <typename Type>
  void *
  alloc(
    Type *t)
  {
    return alloc(sizeof(t));
  }

  Arena();
  ~Arena();

private:
  size_t  len;
  size_t  max_len;
  Block **mem;
};

constexpr size_t DEFAULT_T_MEM_SIZE = 8;

inline AR::Arena::Arena()
{
  this->len     = 1;
  this->max_len = DEFAULT_T_MEM_SIZE;
  this->mem     = (Block **)malloc(sizeof(Block *) * DEFAULT_T_MEM_SIZE);

  Block *block = (Block *)std::malloc(
    sizeof(Block) + sizeof(uint8_t) * AR::BLOCK_DEFAULT_LEN);

  block->max_len = AR::BLOCK_DEFAULT_LEN;
  block->len     = 0;

  this->mem[0] = block;
}

inline AR::Arena::~Arena()
{
  for (size_t i = 0; i < this->len; ++i)
  {
    Block *block = this->mem[i];
    std::free(block);
  }
  std::free(this->mem);

  return;
}

inline void *
AR::Arena::alloc(
  size_t size)
{
  if (!size)
  {
    return nullptr;
  }
now_allocate:
  Block *block       = this->mem[this->len - 1];
  size_t size_of_ptr = sizeof(void *);
  size_t alloc_size  = ((size + size_of_ptr - 1) / size_of_ptr) * size_of_ptr;
  size_t mem_left    = block->max_len - block->len;

  void *ptr = nullptr;

  if (mem_left >= alloc_size)
  {
    ptr = block->mem + block->len;
    block->len += alloc_size;
  }
  else // The current block is full
  {
    if (this->max_len > this->len) // Create a new block
    {
      size_t block_new_size
        = sizeof(Block)
          + sizeof(uint8_t) * std::max(alloc_size, AR::BLOCK_DEFAULT_LEN);

      Block *block   = (Block *)malloc(block_new_size);
      block->len     = 0;
      block->max_len = block_new_size - sizeof(Block);

      this->mem[this->len] = block;
      this->len += 1;

      goto now_allocate;
    }
    else // The aray is full, we need to resize it
    {
      size_t block_new_len = this->len * 2;
      auto   new_mem
        = (Block **)std::realloc(this->mem, block_new_len * sizeof(Block *));

      this->mem     = new_mem;
      this->max_len = block_new_len;

      goto now_allocate;
    }
  }

  return ptr;
}

} // namespace AR

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
    std::string s)
      : m_mem{ s.c_str() }
  {
    this->m_len = s.size();
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
  last()
  {
    return this->m_mem + (this->m_len - 1);
  };
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
    String &other)
  {
    return (this->m_len == other.m_len)
           && (0 == std::memcmp(this->m_mem, other.m_mem, this->m_len));
  }

  bool
  operator!=(
    String &other)
  {
    return !((this->m_len == other.m_len)
             && (0 == std::memcmp(this->m_mem, other.m_mem, this->m_len)));
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

namespace ER
{
constexpr bool TRACE_ENABLE =
#if TRACE_ENABLED
  true;
#else
  false;
#endif

enum class Level
{
  MIN = 0,
  ERROR,
  WARNING,
  INFO,
  MAX,
};

struct E
{
  Level      m_level = Level::MIN;
  size_t     m_type  = 0;
  AR::Arena *m_arena = nullptr;
  void      *m_data  = nullptr;
  ;

  E();
  E(Level level, size_t type, AR::Arena &arena, void *data)
      : m_level{ level },  //
        m_type{ type },    //
        m_arena{ &arena }, //
        m_data{ data }     //
  {};
};

class Events : public UT::Vec<E>
{
public:
  Events(
    AR::Arena &arena)
      : UT::Vec<E>{ arena }
  {
  }
  Events()                    = delete;
  ~Events()                   = default;
  Events(const Events &other) = default;
  Events(
    Events &&other)
  {
    this->m_arena   = other.m_arena;
    this->m_len     = other.m_len;
    this->m_max_len = other.m_max_len;
    this->m_mem     = other.m_mem;

    other.m_mem     = nullptr;
    other.m_len     = 0;
    other.m_max_len = 0;
    other.m_arena   = nullptr;
  }

  using UT::Vec<E>::push;
  using UT::Vec<E>::operator[];
};

#ifdef TRACE_ENABLED

#define UT_BEGIN_TRACE(UT_ARENA, UT_EVENTS, UT_FORMAT, ...)                    \
  ER::Trace trace{ __PRETTY_FUNCTION__, __FILE__, UT_ARENA, UT_EVENTS };       \
  do                                                                           \
  {                                                                            \
    trace.log_entry(__LINE__, UT_FORMAT, __VA_ARGS__);                         \
  } while (false)

#define UT_TRACE(UT_FORMAT, ...) trace.logf(__LINE__, UT_FORMAT, __VA_ARGS__)

#else

#define UT_BEGIN_TRACE(UT_ARENA, UT_EVENTS, UT_FORMAT, ...)
#define UT_TRACE(UT_FORMAT, ...)

#endif

#define UT_TRACE_ENTRY_ARROW " >>> "
#define UT_TRACE_EXIT_ARROW " <<< "
class Trace
{
public:
  const char *m_fn_name;
  const char *m_file_name;
  AR::Arena  *m_arena;
  Events     *m_event_log;

  Trace(
    const char *fn_name,
    const char *file_name,
    AR::Arena  &arena,
    Events     &event_log)
  {
    if (TRACE_ENABLE)
    {
      this->m_fn_name   = fn_name;
      this->m_file_name = file_name;
      this->m_arena     = &arena;
      this->m_event_log = &event_log;
    }
  };

  ~Trace()
  {
    if (TRACE_ENABLE)
    {
      UT::SB sb{};
      sb.concatf(
        UT_TRACE_EXIT_ARROW "%s->%s", this->m_file_name, this->m_fn_name);
      {
        UT::String buffer_copy
          = UT::strdup(*this->m_arena, sb.to_cstr(*this->m_arena));
        E event{ Level::INFO, 0, *this->m_arena, buffer_copy.m_mem };
        this->m_event_log->push(event);
      }
    }
  }

  template <typename... Args>
  void
  log_entry(
    int line, const char *fmt, Args &&...args)
  {
    if (TRACE_ENABLE)
    {
      char  *buffer;
      UT::SB sb{};
      sb.concatf(UT_TRACE_ENTRY_ARROW "%s :: %s \n %d | %s",
                 this->m_file_name,
                 this->m_fn_name,
                 line,
                 fmt);
      (void)asprintf(
        &buffer, sb.to_cstr(*this->m_arena), std::forward<Args>(args)...);
      {
        UT::String buffer_copy = UT::strdup(*this->m_arena, buffer);
        E          event{ Level::INFO, 0, *this->m_arena, buffer_copy.m_mem };
        this->m_event_log->push(event);
      }
      std::free(buffer);
    }
  }

  template <typename... Args>
  void
  logf(
    int line, const char *fmt, Args &&...args)
  {
    if (TRACE_ENABLE)
    {
      char  *buffer;
      UT::SB sb{};
      sb.concatf(" %d | %s", line, fmt);
      (void)asprintf(
        &buffer, sb.to_cstr(*this->m_arena), std::forward<Args>(args)...);
      {
        UT::String buffer_copy = UT::strdup(*this->m_arena, buffer);
        E          event{ Level::INFO, 0, *this->m_arena, buffer_copy.m_mem };
        this->m_event_log->push(event);
      }
      std::free(buffer);
    }
  }
};

} // namespace ER

namespace std
{
inline string
to_string(
  ER::Level type)
{
  switch (type)
  {
  case ER::Level::MIN    : return "MIN";
  case ER::Level::ERROR  : return "ERROR";
  case ER::Level::WARNING: return "WARNING";
  case ER::Level::INFO   : return "INFO";
  case ER::Level::MAX    : return "MAX";
  }

  return "UNREACHABLE";
}
} // namespace std

#endif // UT_HEADER
