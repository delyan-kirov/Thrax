#ifndef ER_HEADER
#define ER_HEADER

#include <cstddef>

#include "AR.hpp"
#include "UT.hpp"

// TODO: Currently only used by LX (Lexer). Intended to be reused by EX
// (Parser) and IT (Interpreter) for unified error reporting.
//
// Known unused parts:
//   - ER::Level::WARNING, INFO, MIN, MAX — only ERROR is used so far
//   - ER::E::m_type — always 0, never branched on
//
// These are kept intentionally for future use.

namespace ER
{

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

#endif // ER_HEADER
