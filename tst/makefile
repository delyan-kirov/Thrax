# Thrax test suite.
#
# Run from the repo root via `make test` / `make native-test` (which build the
# binaries first and delegate here), or directly with `make -C tst`. The recipes
# run from the repo root, so the binaries' relative paths (./dat, ./bin) and the
# runtime sources resolve. The binaries themselves are built by ../makefile.

ROOT   = ..
THRAX  = ./bin/thrax
TESTER = ./bin/tst_all

.PHONY: all test native-test
all: test native-test

# Interpreter smoke test: evaluate every dat/ file through the reified-K machine.
test:
ifdef THRAX_WINDOWS
	cd $(ROOT) && wine $(TESTER)
else
	cd $(ROOT) && $(TESTER)
endif

# Emit, compile and run every example through the native backend and check each
# exits 0 -- the compiled mirror of the interpreter smoke test (the generated
# `THx_force_all` forces every global, so a fault surfaces). The whole IR is
# supported now (subset + FFI + effects); a program is only SKIPped if `thrax -c`
# itself declines it (none in dat/ today).
native-test:
	@cd $(ROOT) && \
	pass=0; skip=0; fail=0; \
	for f in dat/*.thx; do \
	  if $(THRAX) -c "$$f" > tmp.native.c 2>tmp.native.err; then \
	    if cc -O2 tmp.native.c -o tmp.native.bin 2>tmp.native.cc; then \
	      if ./tmp.native.bin >/dev/null 2>&1; then \
	        echo "PASS  $$f"; pass=`expr $$pass + 1`; \
	      else echo "FAIL(run) $$f"; fail=`expr $$fail + 1`; fi; \
	    else echo "FAIL(cc)  $$f"; cat tmp.native.cc; fail=`expr $$fail + 1`; fi; \
	  else echo "SKIP  $$f"; skip=`expr $$skip + 1`; fi; \
	done; \
	rm -f tmp.native.c tmp.native.err tmp.native.cc tmp.native.bin; \
	echo "native-test: pass=$$pass skip=$$skip fail=$$fail"; \
	test $$fail -eq 0
