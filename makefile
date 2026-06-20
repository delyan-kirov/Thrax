# \file makefile
# \note Targets:  make           build thrax + tests (default)
#                 make test      build and run the tests
#                 make clean     remove build artifacts
#                 make format    clang-format the sources
#                 make tokei     line counts
#       Options:  NO_3RD_PARTY=1 build without libffi
#       Options:  THRAX_WINDOWS  build for windows

CXX      = clang++
CXXFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g -Iinc 
LIBS     = 

ifdef THRAX_WINDOWS
NO_3RD_PARTY=1
CXXFLAGS += -D_GNU_SOURCE
LIBS += -static
CXX = x86_64-w64-mingw32-g++
Tthrax = bin/thrax.exe
Ttest  = bin/tst_all.exe
else
Tthrax = bin/thrax
Ttest  = bin/tst_all
endif

ifndef NO_3RD_PARTY
CXXFLAGS += -I$(LIBFFI_DEV)/include
LIBS     += -L$(LIBFFI)/lib -lffi -ldl -Wl,-rpath,$(LIBFFI)/lib
else
CXXFLAGS += -DTHRAX_NO_3RD_PARTY=1
endif

all: $(Tthrax) $(Ttest) compile_flags.txt

LIB_SRCS = $(filter-out src/main.cpp,$(wildcard src/*.cpp)) $(wildcard inc/*.hpp)
bin/UTxAMALG.o: $(LIB_SRCS) | bin ; $(CXX) $(CXXFLAGS) -c src/UTxAMALG.cpp -o $@

TS_LIB_SRCS = $(filter-out tst/tst_all.cpp,$(wildcard tst/*.cpp)) $(wildcard tst/*.hpp)
bin/TS.o: $(TS_LIB_SRCS) | bin ; $(CXX) $(CXXFLAGS) -c tst/TS.cpp -o $@

$(Tthrax):   src/main.cpp    bin/UTxAMALG.o          | bin ; $(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@
$(Ttest): tst/tst_all.cpp bin/TS.o bin/UTxAMALG.o | bin ; $(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@

bin: ; mkdir -p bin

# clangd reads compile_flags.txt; regenerated whenever the flags change
compile_flags.txt: makefile ; @printf '%s\n' $(CXXFLAGS) > $@

.PHONY: test clean format tokei executables valgrind
clean:                ; rm -rf bin tmp.* vgcore* *.orig compile_flags.txt
format:               ; clang-format -i src/*.cpp inc/*.hpp tst/*.cpp tst/*.hpp
tokei:                ; tokei --exclude lib
valgrind: bin/tst_all ; valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes ./bin/tst_all
ifdef THRAX_WINDOWS
test: $(Ttest)        ; wine ./$(Ttest)
else
test: $(Ttest)        ; ./$(Ttest)
endif
