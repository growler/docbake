CROSS_TARGETS := \
	x86_64-unknown-linux-musl \
	x86_64-pc-windows-gnu \
	aarch64-apple-darwin \
	x86_64-apple-darwin

# ---------------------------------------------------------------------------
# Build native binary and run tests
# ---------------------------------------------------------------------------

.PHONY: native
native:
	cargo build --release
	cargo test --release

# ---------------------------------------------------------------------------
# Cross-compile all targets directly on macOS host
# ---------------------------------------------------------------------------

.PHONY: mac-cross
mac-cross:
	@set -eu; \
	if [ "$$(uname -s)" != "Darwin" ]; then \
		echo "mac-cross is intended for macOS hosts"; \
		exit 1; \
	fi; \
	if ! command -v cargo-zigbuild >/dev/null 2>&1; then \
		echo "cargo-zigbuild is required"; \
		exit 1; \
	fi; \
	export SDKROOT="$$(xcrun --sdk macosx --show-sdk-path)"; \
	export CARGO_ZIGBUILD_CACHE_DIR="$(CURDIR)/target/.zigbuild-cache"; \
	mkdir -p "$$CARGO_ZIGBUILD_CACHE_DIR/bin"; \
	ln -sf "$$(which cargo-zigbuild)" "$$CARGO_ZIGBUILD_CACHE_DIR/bin/x86_64-w64-mingw32-dlltool"; \
	export PATH="$$CARGO_ZIGBUILD_CACHE_DIR/bin:$$PATH"; \
	for target in $(CROSS_TARGETS); do \
		echo "--- building $$target ---"; \
		cargo zigbuild --release --target "$$target"; \
	done

# ---------------------------------------------------------------------------
# Build all (native + cross)
# ---------------------------------------------------------------------------

.PHONY: build
build: native mac-cross

# ---------------------------------------------------------------------------
# Create release distribution archives
# ---------------------------------------------------------------------------

.PHONY: dist
dist:
	mkdir -p target/dist
	tar czf target/dist/docbake-linux-amd64.tar.gz \
		--owner=0 --group=0 --mtime='@0' \
		-C target/x86_64-unknown-linux-musl/release \
		docbake
	tar czf target/dist/docbake-darwin-arm64.tar.gz \
		--owner=0 --group=0 --mtime='@0' \
		-C target/aarch64-apple-darwin/release \
		docbake
	tar czf target/dist/docbake-darwin-amd64.tar.gz \
		--owner=0 --group=0 --mtime='@0' \
		-C target/x86_64-apple-darwin/release \
		docbake
	zip -j target/dist/docbake-windows-amd64.zip \
		target/x86_64-pc-windows-gnu/release/docbake.exe

# ---------------------------------------------------------------------------
# Build Debian package
# ---------------------------------------------------------------------------

.PHONY: debian-package
debian-package:
	@set -eu; \
	export SOURCE_DATE_EPOCH=$$(dpkg-parsechangelog -S Date | date -u -f - +%s); \
	dpkg-buildpackage -b -us -uc; \
	mv ../*.deb target/

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

.PHONY: clean-vendor
clean-vendor:
	rm -rf target/cargo-zigbuild target/cargo-zigbuild-*-vendored.tar.xz
