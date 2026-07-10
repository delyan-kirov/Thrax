# Thrax raylib demo -- a standalone little project that exercises the native
# (C) backend's FFI against a real shared library.
#
#   nix develop        # provides raylib (exports $RAYLIB) + cc + make
#   make run           # emit -> compile -> link -> run (needs a display)
#
# Everything is built under ./bin. The demo's `@extern` bindings load
# "bin/libraylib.so", which `make` symlinks to raylib's shared object, so the
# program is self-contained when run from this directory.

THRAX  ?= ../../bin/thrax   # the Thrax compiler (build it from the repo root)
CC     ?= cc
RAYLIB ?=                   # raylib prefix; set by the dev shell (flake.nix)

NAME = raylib_demo

.PHONY: all run clean
all: bin/$(NAME) bin/libraylib.so

bin:
	mkdir -p bin

# Build the compiler from the repo root if it is missing.
$(THRAX):
	$(MAKE) -C ../.. bin/thrax

# Emit self-contained C from the Thrax source, then compile it. FFI programs
# link with -ldl (dlopen/dlsym); the runtime is baked into the emitted C.
bin/$(NAME): MAIN.thx $(THRAX) | bin
	$(THRAX) -c MAIN.thx > bin/$(NAME).c
	$(CC) -O2 bin/$(NAME).c -ldl -o $@

# Symlink raylib's shared object into bin/ so the demo's relative `@extern`
# library path resolves at runtime (its own X11/GL deps resolve via the nix
# rpaths baked into the .so).
bin/libraylib.so: | bin
	@test -n "$(RAYLIB)" || { \
	  echo "RAYLIB is unset. Enter the dev shell first:  nix develop"; \
	  exit 1; }
	ln -sf "$(RAYLIB)/lib/libraylib.so" $@

run: all
	./bin/$(NAME)

clean:
	rm -rf bin
