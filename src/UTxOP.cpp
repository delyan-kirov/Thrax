/*-------------------------------------------------------------------------------
 *\file UTxOP.cpp
 *\info Operator tables -- see UTxOP.hpp.
 *-----------------------------------------------------------------------------*/

#include "UTxOP.hpp"

namespace OP
{

namespace
{

ssize_t
op_neg(
  ssize_t a)
{
  return -a;
}
ssize_t
op_not(
  ssize_t a)
{
  return a == 0 ? 1 : 0;
}

ssize_t
op_add(
  ssize_t a, ssize_t b)
{
  return a + b;
}
ssize_t
op_sub(
  ssize_t a, ssize_t b)
{
  return a - b;
}
ssize_t
op_mul(
  ssize_t a, ssize_t b)
{
  return a * b;
}
ssize_t
op_div(
  ssize_t a, ssize_t b)
{
  UT_FAIL_IF(b == 0);
  return a / b;
}
ssize_t
op_mod(
  ssize_t a, ssize_t b)
{
  UT_FAIL_IF(b == 0);
  return a % b;
}
ssize_t
op_iseq(
  ssize_t a, ssize_t b)
{
  return a == b ? 1 : 0;
}
ssize_t
op_geq(
  ssize_t a, ssize_t b)
{
  return a >= b ? 1 : 0;
}
ssize_t
op_leq(
  ssize_t a, ssize_t b)
{
  return a <= b ? 1 : 0;
}

ssize_t
op_more(
  ssize_t a, ssize_t b)
{
  return a >= b ? 1 : 0;
}
ssize_t
op_less(
  ssize_t a, ssize_t b)
{
  return a <= b ? 1 : 0;
}

} // namespace

const UnaryTable unary_db{
  { NEG, op_neg },
  { NOT, op_not },
};

const BinaryTable binary_db{
  { ADD, op_add },   //
  { SUB, op_sub },   //
  { MUL, op_mul },   //
  { DIV, op_div },   //
  { MOD, op_mod },   //
  { ISEQ, op_iseq }, //
  { GEQ, op_geq },   //
  { LEQ, op_leq },   //
  { MORE, op_more }, //
  { LESS, op_less }, //
};

const ArityTable arity_db{
  { NEG, Arity::Unary },   //
  { NOT, Arity::Unary },   //
  { ADD, Arity::Binary },  //
  { SUB, Arity::Binary },  //
  { MUL, Arity::Binary },  //
  { DIV, Arity::Binary },  //
  { MOD, Arity::Binary },  //
  { ISEQ, Arity::Binary }, //
  { GEQ, Arity::Binary },  //
  { LEQ, Arity::Binary },  //
  { MORE, Arity::Binary }, //
  { LESS, Arity::Binary }, //
  { IF, Arity::Ternary },  //
};

} // namespace OP
