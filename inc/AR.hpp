#ifndef AR_HEADER_
#define AR_HEADER_

#include <cstddef>
#include <cstdint>
#include <cstdlib>
#include <cstring>

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

  // Rewind the arena so every byte it has handed out can be reused, without
  // freeing (and later re-mallocing) the underlying blocks. The blocks stay
  // owned by the arena; only the fill cursors are reset. Any pointers handed
  // out before the reset are dangling afterwards.
  void reset();

  // Eagerly free every block now instead of waiting for the destructor.
  // Idempotent: it flips a flag so the destructor (which also calls this) is a
  // no-op afterwards. Do not allocate from the arena after calling this.
  void dealloc();

  // Object-allocating overloads zero the memory they hand back: the arena
  // gives raw bytes, but callers here are constructing structures and then
  // assigning into them (e.g. `*p = Expr{...}`). std::variant assignment reads
  // the destination's discriminant, so the bytes must start as a valid value
  // (zero == alternative 0 / null / size 0) rather than garbage.
  template <typename Type>
  void *
  alloc()
  {
    void *ptr = alloc(sizeof(Type));
    std::memset(ptr, 0, sizeof(Type));
    return ptr;
  }

  template <typename Type>
  void *
  alloc(
    size_t size)
  {
    size_t bytes = size * sizeof(Type);
    void  *ptr   = alloc(bytes);
    std::memset(ptr, 0, bytes);
    return ptr;
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
  size_t  len;     // number of blocks ever allocated (kept across reset)
  size_t  cur;     // index of the block currently being filled
  size_t  max_len; // capacity of the `mem` array
  Block **mem;
  bool    freed; // true once dealloc() has run; guards a double free

private:
  Arena(const Arena &)            = delete;
  Arena &operator=(const Arena &) = delete;
  Arena(Arena &&)                 = delete;
  Arena &operator=(Arena &&)      = delete;
};

constexpr size_t DEFAULT_T_MEM_SIZE = 8;

} // namespace AR

#endif // AR_HEADER_
