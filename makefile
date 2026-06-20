# \file makefile
# \note Targets:  make           build thrax + tests (default)
#                 make test      build and run the tests
#                 make clean     remove build artifacts
#                 make format    clang-format the sources
#                 make tokei     line counts
#       Options:  NO_3RD_PARTY=1 build without libffi

CXX      = clang++
CXXFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g -Iinc
LIBS     =

# Third-party deps (libffi) are on by default; build with NO_3RD_PARTY=1
# to drop them entirely (FFI calls then abort at runtime).
ifndef NO_3RD_PARTY
CXXFLAGS += -I$(LIBFFI_DEV)/include
LIBS     += -L$(LIBFFI)/lib -lffi -ldl -Wl,-rpath,$(LIBFFI)/lib
else
CXXFLAGS += -DTHRAX_NO_3RD_PARTY=1
endif

all: bin/thrax bin/tst_all compile_flags.txt

LIB_SRCS = $(filter-out src/main.cpp,$(wildcard src/*.cpp)) $(wildcard inc/*.hpp)
bin/UTxAMALG.o: $(LIB_SRCS) | bin ; $(CXX) $(CXXFLAGS) -c src/UTxAMALG.cpp -o $@

TS_LIB_SRCS = $(filter-out tst/tst_all.cpp,$(wildcard tst/*.cpp)) $(wildcard tst/*.hpp)
bin/TS.o: $(TS_LIB_SRCS) | bin ; $(CXX) $(CXXFLAGS) -c tst/TS.cpp -o $@

bin/thrax:   src/main.cpp    bin/UTxAMALG.o          | bin ; $(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@
bin/tst_all: tst/tst_all.cpp bin/TS.o bin/UTxAMALG.o | bin ; $(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@

bin: ; mkdir -p bin

# clangd reads compile_flags.txt; regenerated whenever the flags change
compile_flags.txt: makefile ; @printf '%s\n' $(CXXFLAGS) > $@

.PHONY: test clean format tokei executables valgrind
test: bin/tst_all     ; ./bin/tst_all
clean:                ; rm -rf bin tmp.* vgcore* *.orig compile_flags.txt
format:               ; clang-format -i src/*.cpp inc/*.hpp tst/*.cpp tst/*.hpp
tokei:                ; tokei --exclude lib
valgrind: bin/tst_all ; valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes ./bin/tst_all
