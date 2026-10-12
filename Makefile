# Architectures built by `make`. To support another one, add its triple here or
# override on the command line: make TARGETS=riscv64gc-unknown-linux-musl
# Candidates: rustup target list | grep musl
TARGETS ?= aarch64-unknown-linux-musl x86_64-unknown-linux-musl

BIN  := failsafe
DIST := dist

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml)
DEB_REVISION ?= 1
DEB_VERSION := $(VERSION)-$(DEB_REVISION)

# Target-to-Debian-architecture mapping
deb_arch_aarch64-unknown-linux-musl := arm64
deb_arch_x86_64-unknown-linux-musl  := amd64
deb_arch_armv7-unknown-linux-musleabihf := armhf
deb_arch_riscv64gc-unknown-linux-musl := riscv64
deb_arch = $(or $(deb_arch_$(1)),$(error Unknown Debian architecture for target $(1)))

.PHONY: all test clean deb

all:
	rustup target add $(TARGETS)
	cargo build --release $(addprefix --target ,$(TARGETS))
	mkdir -p $(DIST)
	$(foreach t,$(TARGETS),cp target/$(t)/release/$(BIN) $(DIST)/$(BIN)-$(t);)

deb: all
	mkdir -p $(DIST)
	$(foreach t,$(TARGETS),cargo deb --no-build --target $(t) -o $(DIST)/$(BIN)_$(DEB_VERSION)_$(call deb_arch,$(t)).deb;)

test:
	cargo test

clean:
	cargo clean
	$(RM) -r $(DIST)
