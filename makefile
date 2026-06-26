CXX      = clang++
CXXFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g -Iinc -MMD -MP

Tthrax = bin/thrax
Ttest  = bin/tst_all

ifdef THRAX_3RD_PARTY_ON
CXXFLAGS += -I$(LIBFFI_DEV)/include
LIBS     += -L$(LIBFFI)/lib -lffi -ldl -Wl,-rpath,$(LIBFFI)/lib
CXXFLAGS += -DTHRAX_3RD_PARTY_ON=1
endif

all: $(Tthrax) $(Ttest) compile_flags.txt

# --- PCH Configuration ---
PCH_SRC = inc/UTxAMALG.hpp
PCH_OUT = bin/UTxAMALG.hpp.pch
PCH_FLAGS = -Xclang -include-pch -Xclang $(PCH_OUT)

# Rule to compile PCH binary
$(PCH_OUT): $(PCH_SRC) | bin
	$(CXX) $(CXXFLAGS) -x c++-header $< -o $@

# --- Object rules ---
OBJS = bin/UTxAMALG.o bin/TS.o bin/main.o bin/tst_all.o
-include $(OBJS:.o=.d)

bin/UTxAMALG.o: src/UTxAMALG.cpp $(PCH_OUT) | bin
	$(CXX) $(CXXFLAGS) $(PCH_FLAGS) -c $< -o $@

bin/TS.o: tst/TS.cpp $(PCH_OUT) | bin
	$(CXX) $(CXXFLAGS) $(PCH_FLAGS) -c $< -o $@

bin/main.o: src/main.cpp $(PCH_OUT) | bin
	$(CXX) $(CXXFLAGS) $(PCH_FLAGS) -c $< -o $@

bin/tst_all.o: tst/tst_all.cpp $(PCH_OUT) | bin
	$(CXX) $(CXXFLAGS) $(PCH_FLAGS) -c $< -o $@

# --- Linking Rules ---
$(Tthrax): bin/main.o bin/UTxAMALG.o | bin
	$(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@

$(Ttest): bin/tst_all.o bin/TS.o bin/UTxAMALG.o | bin
	$(CXX) $(CXXFLAGS) $^ $(LIBS) -o $@

bin: ; mkdir -p bin

compile_flags.txt: makefile | bin
	@printf '%s\n' $(CXXFLAGS) > $@
	@printf '%s\n' "-include-pch bin/UTxAMALG.hpp.pch" >> $@

# --- Utility commands ---
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
