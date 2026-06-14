#include "AR.hpp"

namespace AR
{

Arena::Arena()
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

Arena::~Arena()
{
  for (size_t i = 0; i < this->len; ++i)
  {
    Block *block = this->mem[i];
    std::free(block);
  }
  std::free(this->mem);

  return;
}

void *
Arena::alloc(
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
          + sizeof(uint8_t) * std::max(alloc_size, BLOCK_DEFAULT_LEN);

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
