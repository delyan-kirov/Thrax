# \file makefile
# \note Targets:  make           build libs + tests (default)
#                 make test      build and run the tests
#                 make clean     remove build artifacts
#                 make format    clang-format the sources
#                 make tokei     line counts
#       Options:  OPT=-O3        optimization level (default -O0)
#                 NO_3RD_PARTY=1 build without libffi/raylib

CXX      = clang++
OPT     ?= -O0

CXXFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -Wimplicit-fallthrough -g $(OPT) -Iinc
LIBS     =

# Third-party deps (libffi, raylib) are on by default; build with NO_3RD_PARTY=1
# to drop them entirely (FFI calls then abort at runtime).
ifndef NO_3RD_PARTY
CXXFLAGS += -I$(LIBFFI_DEV)/include
LIBS     += -L$(LIBFFI)/lib -lffi -ldl -Wl,-rpath,$(LIBFFI)/lib
else
CXXFLAGS += -DTHRAX_NO_3RD_PARTY=1
endif

SRCS  = $(filter-out src/main.cpp,$(wildcard src/*.cpp))
OBJS  = $(patsubst src/%.cpp,bin/%.o,$(SRCS))
TESTS = $(patsubst tst/%.cpp,bin/%,$(wildcard tst/tst_*.cpp))

# raylib is only reachable through FFI (dlopen), so a NO_3RD_PARTY build skips it.
ifndef NO_3RD_PARTY
RAYLIB_SO = bin/raylib.so
endif

all: bin/libthrax.a bin/thrax.so $(RAYLIB_SO) compile_flags.txt $(TESTS)

# thrax: static + shared from the SAME objects (compiled once, -fPIC)
bin/%.o: src/%.cpp | bin ; $(CXX) $(CXXFLAGS) -fPIC -c $< -o $@
bin/libthrax.a: $(OBJS) ; ar rcs $@ $^
bin/thrax.so:   $(OBJS) ; $(CXX) -shared $^ $(LIBS) -o $@

# tests link the static lib
bin/%: tst/%.cpp bin/libthrax.a makefile ; $(CXX) $(CXXFLAGS) $< bin/libthrax.a $(LIBS) -o $@

# deps copied straight from nix ($RAYLIB comes from the flake's shellHook)
bin/raylib.so: | bin
	@test -n "$(RAYLIB)" || { echo "error: RAYLIB unset — run inside 'nix develop'"; exit 1; }
	cp $(RAYLIB)/lib/libraylib.so $@
bin: ; mkdir -p bin

# clangd reads compile_flags.txt; regenerated whenever the flags change
compile_flags.txt: makefile ; @printf '%s\n' $(CXXFLAGS) > $@

# alias kept so .zed/debug.json's `make executables` build step still works
executables: all

.PHONY: test clean format tokei executables
test: $(TESTS) ; @for t in $(TESTS); do echo "== $$t =="; ./$$t || exit 1; done
clean:  ; rm -rf bin tmp.* vgcore* *.orig compile_flags.txt
format: ; clang-format -i src/*.cpp inc/*.hpp tst/*.cpp tst/*.hpp
tokei:  ; tokei --exclude lib
