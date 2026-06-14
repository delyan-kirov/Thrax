#------------------------------DIRS-----------------------------

SRC = src/
INC = inc/
BIN = bin/
TST = tst/

CFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g 
ifdef GIT_ACTION_CTX
# We need to enable optimization or we wont compile in nix develop shell
CFLAGS += -O1
CFLAGS += -DGIT_ACTION_CTX=1
else
CFLAGS += -O0
endif

CC = clang++ $(CFLAGS) -I$(INC)
CFSO = -fPIC -shared

#------------------------------MAIN-----------------------------
TESTS = tst_mult tst_functional tst_debug

test: $(addprefix $(BIN),$(TESTS))
	@for t in $(TESTS); do $(BIN)$$t; done

test-debug: $(BIN)tst_debug
	@$(BIN)tst_debug

#------------------------------OBJC-----------------------------
THRAXsrc = \
	$(SRC)LX.cpp \
	$(SRC)EX.cpp \
	$(SRC)IT.cpp 

THRAXinc = \
	$(INC)LX.hpp \
	$(INC)UT.hpp \
	$(INC)IT.hpp \
	$(INC)EX.hpp 

THRAX = $(BIN)thrax.so

# $(BIN)main: $(THRAX)
# 	$(CC) $(SRC)main.cpp -o $@ $^

$(THRAX): $(THRAXinc) $(THRAXsrc)
	$(CC) $(CFSO) $(THRAXsrc) $(LIBS) -o $@

#-----------------------------TEST------------------------------

$(BIN)tst_mult: $(TST)tst_mult.cpp $(THRAX)
	$(CC) $(THRAX) $(TST)tst_mult.cpp $(LIBS) -o $@

$(BIN)tst_functional: $(TST)tst_functional.cpp $(THRAX) 
	$(CC) $(THRAX) $(TST)tst_functional.cpp $(LIBS) -o $@

$(BIN)tst_debug: $(TST)tst_debug.cpp $(THRAX) 
	$(CC) $(THRAX) $(TST)tst_debug.cpp $(LIBS) -o $@

#-----------------------------CMND------------------------------
COMMANDS = clean bear test init list format valgrind gf2 executables tokei test-debug clean_wkspace
.PHONY: COMMANDS

executables: $(THRAX)
	@true

debug:
	gf2 $(BIN)tst_mult &

list:
	@true
	$(foreach command, $(COMMANDS), $(info $(command)))

valgrind:
	valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose ./bin/tst_mult
	valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose ./bin/tst_functional
	valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose ./bin/tst_debug

format:
	find . -maxdepth 1 -regex '.*\.\(cpp\|hpp\|c\|h\)$\' -exec clang-format -i {} + 

all:
	make
	make test

tokei:
	tokei --exclude lib

init:
	mkdir -p $(BIN)
	make clean
	make $(BIN)raylib.so
ifndef GIT_ACTION_CTX
	git submodule update --init
	$(MAKE) -C ./lib CC= CXX= CFLAGS= CXXFLAGS= CPPFLAGS= LDFLAGS=
	$(MAKE) -C ./lib BLOBIFY
endif
	make test
	make valgrind

$(BIN)raylib.so:
	cp $(shell find /nix/store -name "libraylib.so" 2>/dev/null | head -1) bin/raylib.so

clean:
	rm -f tmp.*
	rm -f vgcore*
	rm -f *.orig
	rm -rf $(BIN)*
	$(MAKE) -C ./lib clean

clean_wkspace:
	rm -f tmp.*
	rm -f vgcore*
	rm -f *.orig
	$(MAKE) -C ./lib clean

bear:
	mkdir -p $(BIN)
	make clean
	bear -- make init
