# NEXUS OS top-level build.
#
#   make setup   install user-space build prerequisites, fetch + verify Buildroot
#   make build   build Rust components and the full OS image (ISO)
#   make run     boot the live system in QEMU (serial console)
#   make test    Rust unit tests + QEMU boot/installation tests
#
# Everything is built inside this repository (build/, dl/, out/, .tools/).
# Nothing is installed system-wide and no host disk is touched.

SHELL := /bin/bash
TOP := $(CURDIR)

BR_VERSION := 2026.02.3
BR_SHA256 := 5a59e7501b0b4ec52c41f4bfa79412320e0b37eae5f719605a258e8d0c6fc7fb
BR_TARBALL := dl/buildroot-$(BR_VERSION).tar.xz
BR_SRC := build/buildroot-$(BR_VERSION)
BR_OUT := $(TOP)/build/output
BR_DEFCONFIG := nexus_x86_64_defconfig
IMAGES := $(BR_OUT)/images
VERSION := $(shell cat VERSION)
ISO := $(IMAGES)/nexus-$(VERSION)-x86_64.iso

RUST_TARGET := x86_64-unknown-linux-musl
RUST_BINS := nexus nexusd nexus-install

export BR2_DL_DIR := $(TOP)/dl
# Locally built host tools (bc) and shims (wget via curl) only when the host
# lacks them; see scripts/setup-host.sh.
export PATH := $(TOP)/.tools/bin:$(if $(shell command -v wget),,$(TOP)/scripts/hostshims:)$(HOME)/.cargo/bin:$(PATH)

BR := $(MAKE) -C $(BR_SRC) O=$(BR_OUT) BR2_EXTERNAL=$(TOP)/buildroot

.PHONY: setup build rust buildroot-config os iso run run-iso test test-rust test-host test-boot test-install \
	bench fmt lint clean distclean menuconfig linux-menuconfig savedefconfig

setup:
	scripts/setup-host.sh
	$(MAKE) $(BR_SRC)/Makefile

$(BR_TARBALL):
	mkdir -p dl
	curl -fL -o $@.tmp https://buildroot.org/downloads/buildroot-$(BR_VERSION).tar.xz
	echo "$(BR_SHA256)  $@.tmp" | sha256sum -c -
	mv $@.tmp $@

$(BR_SRC)/Makefile: $(BR_TARBALL)
	echo "$(BR_SHA256)  $(BR_TARBALL)" | sha256sum -c -
	mkdir -p build
	tar -xf $(BR_TARBALL) -C build
	touch $@

$(BR_OUT)/.config: buildroot/configs/$(BR_DEFCONFIG) | $(BR_SRC)/Makefile
	$(BR) $(BR_DEFCONFIG)

buildroot-config: $(BR_OUT)/.config

rust:
	cargo build --release --locked --target $(RUST_TARGET)
	mkdir -p out/rust-bin
	for b in $(RUST_BINS); do install -m 0755 target/$(RUST_TARGET)/release/$$b out/rust-bin/$$b; done

build: rust $(BR_OUT)/.config
	$(BR) nexus-tools-dirclean
	$(BR)
	@echo
	@echo "Built: $(ISO)"
	@ls -lh $(IMAGES)/bzImage $(IMAGES)/rootfs.cpio.zst $(IMAGES)/rootfs.squashfs $(ISO)

os: $(BR_OUT)/.config
	$(BR)

menuconfig linux-menuconfig savedefconfig: $(BR_OUT)/.config
	$(BR) $@

run:
	scripts/qemu-run.sh live

run-iso:
	scripts/qemu-run.sh iso

test: test-rust test-host test-boot test-install

test-rust:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	cargo test --locked --workspace -- --test-threads=1

test-host:
	cargo build --locked --workspace
	tests/host/supervisor-test.sh target/debug

test-boot:
	tests/qemu/boot-test.sh $(IMAGES)

test-install:
	tests/qemu/install-test.sh $(ISO)

bench:
	benchmarks/run.sh $(IMAGES)

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

clean:
	cargo clean
	rm -rf out
	-$(BR) clean

distclean: clean
	rm -rf build .tools
