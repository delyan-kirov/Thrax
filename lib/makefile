PROJECT_NAME = Thrax
PROJECT_DIR = /home/dk/Projects/$(PROJECT_NAME)/
PREFIX ?= $(PROJECT_DIR)bin/

FFI_HEADER = $(PROJECT_DIR)inc/ffi.h
FFI_TARGET_HEADER = $(PROJECT_DIR)inc/ffitarget.h

LIBFFI_OBJ = $(PREFIX)lib64/libffi.so
LIBFFI_DIR = $(PROJECT_DIR)lib/libffi/
LIBFFI_BLD = $(LIBFFI_DIR)x86_64-pc-linux-gnu/include/ffi.h
LIBFFI_CONFIG = $(LIBFFI_DIR).configured

RAYLIB_SRC = $(PROJECT_DIR)lib/raylib/
RAYLIB_BLD = $(RAYLIB_SRC)build/
RAYLIB_HEADER = $(PROJECT_DIR)inc/raylib.h
RAYLIB_OBJ = $(PROJECT_DIR)bin/raylib.so

ifdef RAYLIB_ENABLED
raytargets = $(RAYLIB_BLD) $(RAYLIB_HEADER) $(RAYLIB_OBJ) raylib_examples
else
raytargets =
endif

ffitargets = $(LIBFFI_OBJ) $(FFI_HEADER) $(FFI_TARGET_HEADER)

all: $(ffitargets) $(raytargets)

.PHONY: clean
clean:
	rm -rf $(LIBFFI_DIR)x86_64-pc-linux-gnu/
	rm -f $(LIBFFI_DIR).configured
	rm -rf $(RAYLIB_BLD)

$(LIBFFI_CONFIG):
	cd $(LIBFFI_DIR) && ./configure --prefix=$(PREFIX)
	touch $@

$(LIBFFI_BLD): $(LIBFFI_CONFIG)
	$(MAKE) -C $(LIBFFI_DIR)

$(RAYLIB_BLD):
	mkdir -p $(RAYLIB_BLD) \
	&& cd $(RAYLIB_BLD) \
	&& cmake -DPLATFORM=Desktop -DBUILD_SHARED_LIBS=ON .. \
	&& $(MAKE)

$(RAYLIB_HEADER): $(RAYLIB_BLD)
	cp $(RAYLIB_BLD)raylib/include/raylib.h $(RAYLIB_HEADER)

$(RAYLIB_OBJ): $(RAYLIB_BLD)
	cp $(RAYLIB_BLD)raylib/libraylib.so $(RAYLIB_OBJ)

raylib_examples: $(RAYLIB_BLD)
	cd $(PROJECT_DIR)lib/raylib/build \
	&& cmake -DCUSTOMIZE_BUILD=ON -DBUILD_EXAMPLES=ON .. \
	&& $(MAKE)

$(LIBFFI_OBJ): $(LIBFFI_BLD)
	$(MAKE) -C $(LIBFFI_DIR) install

$(FFI_HEADER):
	cd $(PROJECT_DIR) && cp $(PREFIX)include/ffi.h $(FFI_HEADER)

$(FFI_TARGET_HEADER):
	cd $(PROJECT_DIR) && cp $(PREFIX)include/ffitarget.h $(FFI_TARGET_HEADER)
