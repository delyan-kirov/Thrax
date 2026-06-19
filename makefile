# \file makefile
# \note Targets:  make           build thrax + tests (default)
#                 make test      build and run the tests
#                 make clean     remove build artifacts
#                 make format    clang-format the sources
#                 make tokei     line counts
#       Options:  NO_3RD_PARTY=1 build without libffi/raylib

CXX      = clang++
CXXFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g -Iinc
LIBS     =

# Third-party deps (libffi, raylib) are on by default; build with NO_3RD_PARTY=1
# to drop them entirely (FFI calls then abort at runtime).
ifndef NO_3RD_PARTY
CXXFLAGS += -I$(LIBFFI_DEV)/include
LIBS     += -L$(LIBFFI)/lib -lffi -ldl -Wl,-rpath,$(LIBFFI)/lib
# raylib is only reachable through FFI (dlopen), so a NO_3RD_PARTY build skips it.
RAYLIB_SO = bin/raylib.so
else
CXXFLAGS += -DTHRAX_NO_3RD_PARTY=1
endif

all: bin/thrax bin/tst_all $(RAYLIB_SO) compile_flags.txt

LIB_SRCS = $(filter-out src/main.cpp,$(wildcard src/*.cpp)) $(wildcard inc/*.hpp)
bin/UTxAMALG.o: $(LIB_SRCS) | bin
	$(CXX) $(CXXFLAGS) -c src/UTxAMALG.cpp -o $@

bin/thrax:   src/main.cpp    bin/UTxAMALG.o | bin ; $(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@
bin/tst_all: tst/tst_all.cpp bin/UTxAMALG.o $(wildcard tst/*.hpp) | bin ; $(CXX) $(CXXFLAGS) $(filter %.cpp %.o,$^) $(LIBS) -o $@

# raylib copied straight from nix ($RAYLIB comes from the flake's shellHook)
bin/raylib.so: | bin
	@test -n "$(RAYLIB)" || { echo "error: RAYLIB unset — run inside 'nix develop'"; exit 1; }
	cp -n $(RAYLIB)/lib/libraylib.so $@

bin: ; mkdir -p bin

# clangd reads compile_flags.txt; regenerated whenever the flags change
compile_flags.txt: makefile ; @printf '%s\n' $(CXXFLAGS) > $@

.PHONY: test clean format tokei executables valgrind
test: bin/tst_all ; ./bin/tst_all
clean:            ; rm -rf bin tmp.* vgcore* *.orig compile_flags.txt
format:           ; clang-format -i src/*.cpp inc/*.hpp tst/*.cpp tst/*.hpp
tokei:            ; tokei --exclude lib
valgrind:         ; valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose make test
