#include "AR.hpp"
#include <algorithm>

namespace AR
{

// Construction allocates nothing; the block array and first block are created
// lazily on the first alloc() (see below).
Arena::Arena()
{
  this->len     = 0;
  this->cur     = 0;
  this->max_len = 0;
  this->mem     = nullptr;
  this->freed   = false;
}

Arena::~Arena()
{
  this->dealloc();
}

void
Arena::dealloc()
{
  if (this->freed)
  {
    return;
  }

  for (size_t i = 0; i < this->len; ++i)
  {
    std::free(this->mem[i]);
  }
  std::free(this->mem);

  this->mem   = nullptr;
  this->len   = 0;
  this->cur   = 0;
  this->freed = true;
}

void *
Arena::alloc(
  size_t size)
{
  if (!size)
  {
    return nullptr;
  }

  size_t size_of_ptr = sizeof(void *);
  size_t alloc_size  = ((size + size_of_ptr - 1) / size_of_ptr) * size_of_ptr;

  // Lazily create the block-pointer array on first use. This also re-arms the
  // arena after a dealloc() (which leaves mem == nullptr, freed == true).
  if (!this->mem)
  {
    this->max_len = DEFAULT_T_MEM_SIZE;
    this->mem     = (Block **)std::malloc(sizeof(Block *) * this->max_len);
    this->len     = 0;
    this->cur     = 0;
    this->freed   = false;
  }

now_allocate:
  if (this->cur < this->len) // We have a current block
  {
    Block *block    = this->mem[this->cur];
    size_t mem_left = block->max_len - block->len;

    if (mem_left >= alloc_size)
    {
      void *ptr = block->mem + block->len;
      block->len += alloc_size;
      return ptr;
    }

    // The current block is full; reuse a spare left behind by reset().
    if (this->cur + 1 < this->len)
    {
      this->cur += 1;
      goto now_allocate;
    }
  }

  // No usable block: the arena is empty or every existing block is full.
  if (this->len < this->max_len) // Room in the array for a new block
  {
    size_t block_new_size
      = sizeof(Block)
        + sizeof(uint8_t) * std::max(alloc_size, BLOCK_DEFAULT_LEN);

    Block *block   = (Block *)std::malloc(block_new_size);
    block->len     = 0;
    block->max_len = block_new_size - sizeof(Block);

    this->mem[this->len] = block;
    this->cur            = this->len;
    this->len += 1;

    goto now_allocate;
  }
  else // The array is full, we need to resize it
  {
    size_t block_new_len = this->max_len * 2;
    auto   new_mem
      = (Block **)std::realloc(this->mem, block_new_len * sizeof(Block *));

    this->mem     = new_mem;
    this->max_len = block_new_len;

    goto now_allocate;
  }
}

void
Arena::reset()
{
  for (size_t i = 0; i < this->len; ++i)
  {
    this->mem[i]->len = 0;
  }
  this->cur = 0;
}

} // namespace AR
