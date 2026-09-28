# Sandy — local helpers (Rust crate root).
#
# Tests:
#   make test              # cargo test + pytest
#   make test-rust         # cargo test only
#   make test-python       # pytest only (uv --extra dev)
#
# Optional: CARGO_TEST_FLAGS, PYTEST_ARGS

CARGO_TEST_FLAGS ?=
PYTEST_ARGS ?=
FIRECRACKER_VERSION ?= 1.11.0
FIRECRACKER_ARCH ?= x86_64
FIRECRACKER_INSTALL_DIR ?= $(HOME)/.local/bin
FIRECRACKER_TARGET ?= x86_64-unknown-linux-musl
FIRECRACKER_KERNEL_VERSION ?=

.PHONY: help test test-rust test-python firecracker-install firecracker-kernel firecracker-rootfs

help: ## Show targets
	@echo "Sandy Makefile"
	@echo ""
	@echo "  make test               Run Rust (cargo test) and Python (pytest)"
	@echo "  make test-rust          cargo test only (optional: CARGO_TEST_FLAGS=...)"
	@echo "  make test-python        pytest only (uv run --extra dev)"
	@echo "  make firecracker-install Install Firecracker and fetch its kernel"
	@echo "  make firecracker-kernel Fetch the current Firecracker CI kernel"
	@echo "  make firecracker-rootfs Build the Firecracker ext4 rootfs (requires root)"

test: test-rust test-python ## Run Rust and Python tests

test-rust: ## Run cargo test
	cargo test $(CARGO_TEST_FLAGS)

test-python: ## Run pytest (needs uv; installs dev extra on first run)
	uv run --extra dev pytest $(PYTEST_ARGS)

firecracker-install: firecracker-kernel ## Install Firecracker and fetch the default kernel
	FIRECRACKER_VERSION="$(FIRECRACKER_VERSION)" FIRECRACKER_ARCH="$(FIRECRACKER_ARCH)" FIRECRACKER_INSTALL_DIR="$(FIRECRACKER_INSTALL_DIR)" .firecracker/install.sh

firecracker-kernel: ## Fetch the current Firecracker CI kernel
	FIRECRACKER_ARCH="$(FIRECRACKER_ARCH)" FIRECRACKER_KERNEL_VERSION="$(FIRECRACKER_KERNEL_VERSION)" .firecracker/fetch-kernel.sh

firecracker-rootfs: ## Build the Firecracker ext4 rootfs
	rustup target add $(FIRECRACKER_TARGET)
	cargo build --release --no-default-features --features firecracker --target $(FIRECRACKER_TARGET)
	if [ "$$(id -u)" -eq 0 ]; then \
		FIRECRACKER_TARGET="$(FIRECRACKER_TARGET)" .firecracker/build-rootfs.sh; \
	else \
		FIRECRACKER_TARGET="$(FIRECRACKER_TARGET)" sudo -E .firecracker/build-rootfs.sh; \
	fi
