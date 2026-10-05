# Architectures built by `make`. To support another one, add its triple here or
# override on the command line: make TARGETS=riscv64gc-unknown-linux-musl
# Candidates: rustup target list | grep musl
TARGETS ?= aarch64-unknown-linux-musl x86_64-unknown-linux-musl

BIN  := failsafe
DIST := dist

.PHONY: all test clean

all:
	rustup target add $(TARGETS)
	cargo build --release $(addprefix --target ,$(TARGETS))
	mkdir -p $(DIST)
	$(foreach t,$(TARGETS),cp target/$(t)/release/$(BIN) $(DIST)/$(BIN)-$(t);)

test:
	cargo test

clean:
	cargo clean
	$(RM) -r $(DIST)
