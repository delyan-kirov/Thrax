#include "EX.hpp"
#include "LX.hpp"
#include "TL.hpp"
#include "UT.hpp"
#include <cstdio>
#include <utility>

#define TSTxCTL 0

namespace TDATA
{
using INPUTS_t = std::pair<const char *, int>;

constexpr INPUTS_t INPUTS[] = {
  // TODO: Split test, add more data
  { "1 / 2", 0 },
  { "2 * 2", 4 },
  { "1 - 1 - 2", -2 },
  { "1 + 2 * 2", 5 },
  { "-2", -2 },
  { "(1 + 2) * 2", 6 },
  { "1", 1 },
  { "(1)", 1 },
  { "1 + (1 + 1) + 1", 4 },
  { "(1 - 2) - (3 - 1)", -3 },
  { "1 - 2", -1 },
  { "2 * 3 * 4", 24 },
  { "2 * -3 * 4", -24 },
  { "2 * -3", -6 },
  { "2 + -3", -1 },
  { "-1", -1 },
  { "-1 + 2", 1 },
  { "- (-(1 + 2))", 3 },
  { "1 - (2 - (3 + 43)) + 4 - ( 1 + 3   )", 45 },
  { "10 - (5 - 2)", 7 },
  { "(1 + 2) + (3 + 4)", 10 },
  { "(((((5)))))", 5 },
  { "-5 + ((((-(-5)))))", 0 },
  { "0 - 1", -1 },
  { "0 - (1 + 2 + 3)", -6 },
  { "(8 - 3) - (2 - 1)", 4 },
  { "42", 42 },
  { "(7 + 3) - (2 + 1)", 7 },
  { "1 - (2 - (3 - (4 - 5)))", 3 },
  { "1 - 2 - 3 - 4", -8 },
  { "(1 - 2) - (3 - 4)", 0 },
  { "100 - (50 + 25)", 25 },
  { "(100 - 50) + 25", 75 },
  { "1 + (2 + (3 + (4 + 5)))", 15 },
  { "1 - (2 + (3 + (4 + 5)))", -13 },
  { "(1 + 2) - (3 + (4 - 5))", 1 },
  { "((1 + 2) - 3) + (4 - 5)", -1 },
  { "0 - (0 - (0 - (0 - 1)))", 1 },
  { "5 % 3 + 1", 3 },
  { "3 % 2 % 6", 1 },
  { "12 / 3 / 2 + 1", 3 },
  { "3 - (-(1 + 2))", 6 },
  { "3 * 5 * 3", 45 },
  { "5 % - 3 + 1", 3 },
  { "1 - 2 - (3 - (4 + (5 - (6 + (7 - (8 - (9 + (10 - (11 + 12)))))))))", 4 },
  { "10 - 5 - 2", 3 },
  { "(1 + (1 + 2))", 4 },
  { "1 - 2 * (3 * 3) + 5 * -3", -32 },
  { "-1 - 2 * 3 + 3", -4 },
  { "2 * - (-3)", 6 },
  { "1 - (2 - 3) - 4", -2 },
  { "2 - 3 * 4 + 1", -9 },
  { "1 + 1 + (1 + 1) + (((1))) + (((1) + 1))", 7 },
  { "1 - 2 - 3", -4 },
  { "((3*(244 - 57 + 4)*(244 - 57 + 4) - (74 - 777)) / (5 - (44 + 77 - "
    "47)*(44 + 77 - 47)*(44 + 77 - 47)) + ((4 + 27)*(3 - 7)))",
    -124 },
  { "3 * 2 + 2 / 2 * 2 + 1", 9 },
  { "(((7 * (3 + 2)) - (5 * (2 + (6 * 1)))) * ((4 + (9 * (3 - 1))) - (2 * (1 "
    "+ 3)))) + (((12 * (8 - (3 * (2 + 1)))) - (5 * (6 - (2 * (1 + (2 * "
    "2)))))) * (3 + (4 * (2 + (1 * 5)))))",
    178 },
  { "(3 + ((((((1)) + 1))))) - (-(1 + 2)) - ((((1))))", 7 },
  { "1+ 2", 3 },
  { "(1 + 2) + -1 * (1 * (2 * -1) * 1 + ((1 * 1)))", 4 },
#if false
#endif

};
} // namespace TDATA

namespace
{
bool
run()
{
  for (size_t i = 0; i < ARRAY_LEN(TDATA::INPUTS); ++i)
  {
    auto         tdata = TDATA::INPUTS[i];
    AR::Arena    arena{};
    const char  *input      = tdata.first;
    const size_t input_size = std::strlen(input);
    LX::Lexer    l{ UT::String{ input, input_size }, arena, 0, input_size };

    (void)l.run();
    l.generate_event_report();
    EX::Parser parser{ l };
    parser.run();

    TL::Instance instance{ *parser.m_exprs.begin(), TL::Env{} };
    TL::Instance result = TL::eval(instance);

    if (EX::Type::Int == result.m_expr.m_type)
    {
      if (tdata.second != result.m_expr.as.m_int)
      {
        UT_FAIL_MSG("Expected %s but found %s, expression number %zu",
                    UT_TCS(tdata.second),
                    UT_TCS(result.m_expr.as.m_int),
                    i);
      }
    }
  }

  return true;
}
} // namespace

int
main()
{
  if (!run())
  {
    std::printf("%s [OK]\n", __FILE_NAME__);
    return 1;
  }
  else
  {
    std::printf("TEST: %s->OK\n", __FILE_NAME__);
  }
}
