#include <cstddef>
#include <cstdio>
#include <cstring>

#include "AR.hpp"
#include "EX.hpp"

namespace
{

bool
tst_allocating_exprs()
{
  return true;
}

bool
tst_multiple_big_allocation(
  void)
{
  AR::T            arena{};
  std::string      msg                = "INFO: " + std::string(__func__) + " ";
  constexpr size_t num_of_allocations = 100;

  char *new_msg = nullptr;
  for (size_t i = 0; i < num_of_allocations; ++i)
  {
    new_msg = (char *)arena.alloc(100000000);
    std::strcpy(new_msg, msg.c_str());
  }

  std::printf("%s(%p)\n", new_msg, new_msg);

  return true;
}

bool
tst_alloc_of_a_word(
  void)
{
  AR::T arena{};

  auto word = (size_t *)arena.alloc<size_t>();
  *word     = 69;
  std::printf("INFO: %ld(%p)", *word, word);

  return true;
}
} // namespace

int
main()
{
  if (!tst_multiple_big_allocation())
  {
    return -1;
  }
  if (!tst_alloc_of_a_word())
  {
    return -1;
  }
  if (!tst_allocating_exprs())
  {
    return -1;
  }
}
