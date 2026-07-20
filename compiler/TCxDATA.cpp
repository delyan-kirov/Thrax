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
  const char *name, const char *I)
{
  return {
    { { I, I, I }, OP::mono(name, I) },
    { { OP::TY_REAL, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { I, OP::TY_REAL, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, I, OP::TY_REAL }, OP::mono(name, OP::TY_REAL) },
  };
}

std::vector<Overload>
cmp(
  const char *name, const char *I)
{
  const char *B = OP::TY_BOOL;
  return {
    { { I, I, B }, OP::mono(name, I) },
    { { OP::TY_REAL, OP::TY_REAL, B }, OP::mono(name, OP::TY_REAL) },
    { { I, OP::TY_REAL, B }, OP::mono(name, OP::TY_REAL) },
    { { OP::TY_REAL, I, B }, OP::mono(name, OP::TY_REAL) },
  };
}

// Equality: the numeric comparisons plus Str byte-equality (`?=` on two strings
// -> Bool). Backs exact string pattern matching (see
// doc/strings-and-arrays.md).
std::vector<Overload>
eq(
  const char *I)
{
  std::vector<Overload> v = cmp(OP::ISEQ, I);
  v.push_back({ { OP::TY_STR, OP::TY_STR, OP::TY_BOOL },
                OP::mono(OP::ISEQ, OP::TY_STR) });
  return v;
}

} // namespace

OverloadTable
make_overload_db(
  const char *I)
{
  return OverloadTable{
    { OP::ADD, arith(OP::ADD, I) },
    { OP::SUB, arith(OP::SUB, I) },
    { OP::MUL, arith(OP::MUL, I) },
    { OP::DIV, arith(OP::DIV, I) },
    { OP::MOD, arith(OP::MOD, I) },
    { OP::ISEQ, eq(I) },
    { OP::GEQ, cmp(OP::GEQ, I) },
    { OP::LEQ, cmp(OP::LEQ, I) },
    { OP::MORE, cmp(OP::MORE, I) },
    { OP::LESS, cmp(OP::LESS, I) },
    { OP::NEG,
      { { { I, I }, OP::mono(OP::NEG, I) },
        { { OP::TY_REAL, OP::TY_REAL }, OP::mono(OP::NEG, OP::TY_REAL) } } },
    { OP::NOT, { { { OP::TY_BOOL, OP::TY_BOOL }, OP::mono(OP::NOT, I) } } },
    // `++` concatenation: Str x Str -> Str and Array x Array -> Array, both
    // implemented by one byte-concat (OP::CONCAT_IMPL).
    { OP::CONCAT,
      { { { OP::TY_STR, OP::TY_STR, OP::TY_STR }, OP::CONCAT_IMPL },
        { { OP::TY_ARRAY, OP::TY_ARRAY, OP::TY_ARRAY }, OP::CONCAT_IMPL } } },

    // The growable-byte-vector ops, overloaded so they work on both Str and
    // Array (nominally distinct, one runtime rep). Each variant maps to the
    // same impl (the reserved name); the resolver discriminates on the first
    // operand, and mutators return the same kind they took. A byte is an Int
    // (0..255).
    { OP::ARR_LEN,
      { { { OP::TY_ARRAY, I }, OP::ARR_LEN },
        { { OP::TY_STR, I }, OP::ARR_LEN } } },
    { OP::ARR_CAP,
      { { { OP::TY_ARRAY, I }, OP::ARR_CAP },
        { { OP::TY_STR, I }, OP::ARR_CAP } } },
    { OP::ARR_GET,
      { { { OP::TY_ARRAY, I, I }, OP::ARR_GET },
        { { OP::TY_STR, I, I }, OP::ARR_GET } } },
    { OP::ARR_PUSH,
      { { { OP::TY_ARRAY, I, OP::TY_ARRAY }, OP::ARR_PUSH },
        { { OP::TY_STR, I, OP::TY_STR }, OP::ARR_PUSH } } },
    { OP::ARR_SET,
      { { { OP::TY_ARRAY, I, I, OP::TY_ARRAY }, OP::ARR_SET },
        { { OP::TY_STR, I, I, OP::TY_STR }, OP::ARR_SET } } },
    { OP::ARR_SLICE,
      { { { OP::TY_ARRAY, I, I, OP::TY_ARRAY }, OP::ARR_SLICE },
        { { OP::TY_STR, I, I, OP::TY_STR }, OP::ARR_SLICE } } },
  };
}

} // namespace TC
