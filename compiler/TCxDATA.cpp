/*-------------------------------------------------------------------------------
 *\file TCxDATA.cpp
 *\info The typed operator overload table (see TCxDATA.hpp). Kept out of the
 *      header because only this datum needs OP's type-name constants and impl
 *      keys; the rest of TC's data definitions are header-only.
 *-----------------------------------------------------------------------------*/

#include "TCxDATA.hpp"
#include "OP.hpp"

namespace TC
{

/*------------------------------------------------------------------------------
 *\GLOBALS
 *-----------------------------------------------------------------------------*/

void
Globals::add(
  GlobalEntry e)
{
  m_index.emplace(std::string(e.def->name), m_entries.size());
  m_entries.push_back(std::move(e));
}

GlobalEntry *
Globals::find(
  UT::Vu name)
{
  auto it = m_index.find(std::string(name));
  return it == m_index.end() ? nullptr : &m_entries[it->second];
}

// source-order iteration (Phases B and C)
GlobalStore::iterator
Globals::begin()
{
  return m_entries.begin();
}

GlobalStore::iterator
Globals::end()
{
  return m_entries.end();
}

/*------------------------------------------------------------------------------
 *\OVERLOADS
 *-----------------------------------------------------------------------------*/

namespace
{

// Arithmetic: Int x Int -> Int, otherwise Real (mixed operands coerce).
std::vector<Overload>
arith(
  const char *name)
{
  return {
    { { OP::TY_INT, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_INT) },
    { { OP::TY_REAL, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_INT, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, OP::TY_INT, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
  };
}

// Comparison: any Int/Real combination -> Int.
std::vector<Overload>
cmp(
  const char *name)
{
  return {
    { { OP::TY_INT, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_INT) },
    { { OP::TY_REAL, OP::TY_REAL, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_INT, OP::TY_REAL, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, OP::TY_INT, OP::TY_INT }, OP::mono(name, OP::TY_REAL) },
  };
}

} // namespace

const OverloadTable overload_db{
  { OP::ADD, arith(OP::ADD) },
  { OP::SUB, arith(OP::SUB) },
  { OP::MUL, arith(OP::MUL) },
  { OP::DIV, arith(OP::DIV) },
  { OP::MOD, arith(OP::MOD) },
  { OP::ISEQ, cmp(OP::ISEQ) },
  { OP::GEQ, cmp(OP::GEQ) },
  { OP::LEQ, cmp(OP::LEQ) },
  { OP::MORE, cmp(OP::MORE) },
  { OP::LESS, cmp(OP::LESS) },
  { OP::NEG,
    { { { OP::TY_INT, OP::TY_INT }, OP::mono(OP::NEG, OP::TY_INT) },
      { { OP::TY_REAL, OP::TY_REAL }, OP::mono(OP::NEG, OP::TY_REAL) } } },
  { OP::NOT,
    { { { OP::TY_INT, OP::TY_INT }, OP::mono(OP::NOT, OP::TY_INT) } } },
};

} // namespace TC
