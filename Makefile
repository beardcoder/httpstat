# Build & release helpers for httpstat (Rust).
#
# Linux targets use `cross` (Docker-based) so they build cleanly from macOS;
# the rustls/ring TLS stack is pure-Rust, so the musl targets link statically
# with no OpenSSL dependency. macOS targets build with the native toolchain.

BIN        := httpstat
DIST       := dist

LINUX_TARGETS  := x86_64-unknown-linux-musl aarch64-unknown-linux-musl
MACOS_TARGETS  := x86_64-apple-darwin aarch64-apple-darwin

.PHONY: help build test fmt clippy clean \
        build-all build-linux build-macos install-targets dist

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Build a release binary for the host
	cargo build --release

test: ## Run the test suite
	cargo test

fmt: ## Format the code
	cargo fmt

clippy: ## Lint with clippy (warnings as errors)
	cargo clippy --all-targets -- -D warnings

clean: ## Remove build artifacts
	cargo clean
	rm -rf $(DIST)

install-targets: ## Add the rustup targets needed for cross-compilation
	rustup target add $(LINUX_TARGETS) $(MACOS_TARGETS)

build-linux: ## Cross-compile Linux binaries (requires `cross` + Docker)
	@command -v cross >/dev/null || { echo "install cross: cargo install cross"; exit 1; }
	@for t in $(LINUX_TARGETS); do \
		echo "==> $$t"; \
		cross build --release --target $$t; \
	done

build-macos: ## Build macOS binaries (native toolchain)
	@for t in $(MACOS_TARGETS); do \
		echo "==> $$t"; \
		rustup target add $$t >/dev/null 2>&1 || true; \
		cargo build --release --target $$t; \
	done

build-all: build-linux build-macos dist ## Build all platform binaries and collect them

dist: ## Collect built binaries into dist/ as tarballs
	@mkdir -p $(DIST)
	@for t in $(LINUX_TARGETS) $(MACOS_TARGETS); do \
		src=target/$$t/release/$(BIN); \
		if [ -f $$src ]; then \
			echo "==> packaging $$t"; \
			tar -czf $(DIST)/$(BIN)-$$t.tar.gz -C target/$$t/release $(BIN); \
		fi; \
	done
	@echo "Artifacts in $(DIST)/:" && ls -1 $(DIST) 2>/dev/null || true
