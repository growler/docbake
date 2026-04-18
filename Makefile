MANIFEST     := builder.toml
DEBOSTREE    := deb-ostree -m $(MANIFEST)

CROSS_TARGETS := \
	x86_64-unknown-linux-musl \
	x86_64-pc-windows-gnu \
	aarch64-apple-darwin \
	x86_64-apple-darwin

CONTAINER_RUN := ostree-podman run --rm \
	--volume $(CURDIR):/root/src \
	--env PATH=/usr/local/bin:/usr/opt/zig-x86_64-linux-0.16.0:/usr/bin:/bin \
	--rootfs $$($(DEBOSTREE) spec build-ref):O

# ---------------------------------------------------------------------------
# Build the deb-ostree tree
# ---------------------------------------------------------------------------

.PHONY: build-tree
build-tree:
	scripts/build-builder

# ---------------------------------------------------------------------------
# Build packages in a container
# ---------------------------------------------------------------------------

.PHONY: build
build:
	$(CONTAINER_RUN) /bin/sh -c '\
		set -eu; \
		cd /root/src; \
		echo "--- building native (x86_64-unknown-linux-gnu) ---"; \
		cargo build --release; \
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
# Build Debian package
# ---------------------------------------------------------------------------

.PHONY: debian-package
debian-package:
	$(CONTAINER_RUN) /bin/sh -c '\
		set -eu; \
		cd /root/src; \
		export SOURCE_DATE_EPOCH=$$(dpkg-parsechangelog -S Date | date -u -f - +%s); \
		find debian -exec touch -d @$$SOURCE_DATE_EPOCH {} +; \
		dpkg-buildpackage -b -us -uc; \
		mv /root/src/../*.deb /root/src/target \
	'

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

.PHONY: clean-vendor
clean-vendor:
	rm -rf target/cargo-zigbuild target/cargo-zigbuild-*-vendored.tar.xz
