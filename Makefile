MANIFEST     := server/builder.toml
DEBOSTREE    := deb-ostree -m $(MANIFEST)

CROSS_TARGETS := \
	x86_64-unknown-linux-musl \
	x86_64-pc-windows-gnu \
	aarch64-apple-darwin \
	x86_64-apple-darwin

BUILDER_REF    = $$($(DEBOSTREE) spec build-ref)

CONTAINER_RUN := ostree-podman run --rm \
	--volume $(CURDIR):/root/src \
	--env PATH=/usr/local/bin:/usr/bin:/bin \
	--rootfs $(BUILDER_REF):O

CONTAINER_RUN_CROSS := ostree-podman run --rm \
	--volume $(CURDIR):/root/src \
	--env PATH=/usr/local/bin:/usr/opt/zig-x86_64-linux-0.16.0:/usr/bin:/bin \
	--rootfs $(BUILDER_REF):O

# ---------------------------------------------------------------------------
# Build the deb-ostree tree
# ---------------------------------------------------------------------------

.PHONY: build-tree
build-tree:
	scripts/build-builder

# ---------------------------------------------------------------------------
# Build native binary and run tests
# ---------------------------------------------------------------------------

.PHONY: native
native:
	$(CONTAINER_RUN) /bin/sh -c '\
		set -eu; \
		cd /root/src; \
		echo "--- build (native) ---"; \
		cargo build --release; \
		echo "--- test ---"; \
		cargo test --release \
	'

# ---------------------------------------------------------------------------
# Cross-compile all targets
# ---------------------------------------------------------------------------

.PHONY: cross
cross:
	$(CONTAINER_RUN_CROSS) /bin/sh -c '\
		set -eu; \
		cd /root/src; \
		export SDKROOT=/usr/opt/MacOSX11.3.sdk; \
		export CARGO_ZIGBUILD_CACHE_DIR=/root/src/target/.zigbuild-cache; \
		mkdir -p "$$CARGO_ZIGBUILD_CACHE_DIR/bin"; \
		ln -sf "$$(which cargo-zigbuild)" "$$CARGO_ZIGBUILD_CACHE_DIR/bin/x86_64-w64-mingw32-dlltool"; \
		export PATH="$$CARGO_ZIGBUILD_CACHE_DIR/bin:$$PATH"; \
		for target in $(CROSS_TARGETS); do \
			echo "--- building $$target ---"; \
			cargo zigbuild --release --target "$$target"; \
		done \
	'

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
		echo "cargo-zigbuild is required (run inside the nix devshell)"; \
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
build: native cross

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
	$(CONTAINER_RUN) /bin/sh -c '\
		set -eu; \
		cd /root/src/target/x86_64-pc-windows-gnu/release; \
		zip -j /root/src/target/dist/docbake-windows-amd64.zip docbake.exe \
	'

# ---------------------------------------------------------------------------
# Build Debian package
# ---------------------------------------------------------------------------

.PHONY: debian-package
debian-package:
	$(CONTAINER_RUN) /bin/sh -c '\
		set -eu; \
		cd /root/src; \
		export SOURCE_DATE_EPOCH=$$(dpkg-parsechangelog -S Date | date -u -f - +%s); \
		dpkg-buildpackage -b -us -uc; \
		mv /root/src/../*.deb /root/src/target \
	'

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

.PHONY: clean-vendor
clean-vendor:
	rm -rf target/cargo-zigbuild target/cargo-zigbuild-*-vendored.tar.xz
