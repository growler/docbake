MANIFEST     := builder.toml
DEBOSTREE    := deb-ostree -m $(MANIFEST)

ZIGBUILD_VERSION := 0.22.1
ZIGBUILD_VENDOR  := target/cargo-zigbuild
ZIGBUILD_TARBALL := target/cargo-zigbuild-$(ZIGBUILD_VERSION)-vendored.tar.xz

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
# Vendor cargo-zigbuild
# ---------------------------------------------------------------------------

$(ZIGBUILD_VENDOR)/vendor/cargo-zigbuild:
	mkdir -p $(ZIGBUILD_VENDOR)
	cd $(ZIGBUILD_VENDOR) && cargo init --name _stub . 2>/dev/null || true
	printf '[dependencies]\ncargo-zigbuild = "=%s"\n' $(ZIGBUILD_VERSION) \
		>> $(ZIGBUILD_VENDOR)/Cargo.toml
	cargo generate-lockfile --manifest-path $(ZIGBUILD_VENDOR)/Cargo.toml
	cargo vendor --manifest-path $(ZIGBUILD_VENDOR)/Cargo.toml $(ZIGBUILD_VENDOR)/vendor
	rm -f $(ZIGBUILD_VENDOR)/Cargo.toml $(ZIGBUILD_VENDOR)/Cargo.lock
	rm -rf $(ZIGBUILD_VENDOR)/src
	mkdir -p $(ZIGBUILD_VENDOR)/.cargo
	printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' \
		> $(ZIGBUILD_VENDOR)/.cargo/config.toml

$(ZIGBUILD_TARBALL): $(ZIGBUILD_VENDOR)/vendor/cargo-zigbuild
	tar -cJf $@ -C target cargo-zigbuild

.PHONY: vendor-zigbuild
vendor-zigbuild: $(ZIGBUILD_TARBALL)

# ---------------------------------------------------------------------------
# Build the deb-ostree tree
# ---------------------------------------------------------------------------

.PHONY: build-tree
build-tree: $(ZIGBUILD_TARBALL)
	$(DEBOSTREE) update
	$(DEBOSTREE) build

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
		export CARGO_ZIGBUILD_CACHE_DIR=/tmp/zigbuild-cache; \
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
		dpkg-buildpackage -b -us -uc; \
		echo "--- .deb packages ---"; \
		ls -lh /root/src/../*.deb \
	'

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------

.PHONY: clean-vendor
clean-vendor:
	rm -rf $(ZIGBUILD_VENDOR) $(ZIGBUILD_TARBALL)
