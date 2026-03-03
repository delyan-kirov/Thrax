#------------------------------DIRS-----------------------------

SRC = src/
INC = inc/
BIN = bin/
TST = tst/

CFLAGS = -Wall -Wextra -Wimplicit-fallthrough -Werror -g -O1 
ifdef GIT_ACTION_CTX
CFLAGS += -DGIT_ACTION_CTX=1
else
RAYLIB_ENABLED ?= 1
export RAYLIB_ENABLED
endif

CC = clang++ $(CFLAGS) -I$(INC)
CFSO = -fPIC -shared

ifdef RAYLIB_ENABLED
LIBS = -L./bin/lib64 -lffi -Wl,-rpath,'$$ORIGIN/lib64' -L./bin/
# LIBS = -lffi -lraylib -Wl,-rpath,'$$ORIGIN/lib'
else ifdef GIT_ACTION_CTX
LIBS = -lffi -Wl,-rpath,'$$ORIGIN/lib64'
else
LIBS = -L./bin/lib64 -lffi -Wl,-rpath,'$$ORIGIN/lib64'
endif

#------------------------------MAIN-----------------------------
# 
test: $(BIN)tst_mult $(BIN)tst_functional
	@$(BIN)tst_mult
	@$(BIN)tst_functional

#------------------------------OBJC-----------------------------
THRAXsrc = \
	$(SRC)LX.cpp \
	$(SRC)EX.cpp \
	$(SRC)TL.cpp

THRAXinc = \
	$(INC)LX.hpp \
	$(INC)UT.hpp \
	$(INC)EX.hpp \
	$(INC)TL.hpp

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

#-----------------------------CMND------------------------------
COMMANDS = clean bear test init list format valgrind gf2 trace executables tokei
.PHONY: COMMANDS

executables: $(THRAX)
	@true

trace: CFLAGS += -DTRACE_ENABLED
trace: clean $(THRAX)
	make

debug:
	gf2 $(BIN)tst_mult &

list:
	@true
	$(foreach command, $(COMMANDS), $(info $(command)))

valgrind:
	valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose ./bin/tst_mult
	valgrind --leak-check=full --show-leak-kinds=all --track-origins=yes --verbose ./bin/tst_functional

format:
	find . -regex '.*\.\(cpp\|hpp\|c\|h\)$\' -exec clang-format -i {} + 

all:
	make
	make test

tokei:
	tokei --exclude lib

init:
	mkdir -p $(BIN)
	make clean
ifndef GIT_ACTION_CTX
	git submodule update --init
	$(MAKE) -C ./lib CC= CXX= CFLAGS= CXXFLAGS= CPPFLAGS= LDFLAGS=
endif
	make test
	make valgrind

clean:
	rm -f tmp.*
	rm -f vgcore*
	rm -rf $(BIN)*
	$(MAKE) -C ./lib clean

clean_wkspace:
	rm -f tmp.*
	rm -f vgcore*
	$(MAKE) -C ./lib clean

bear:
	mkdir -p $(BIN)
	make clean
	bear -- make init
