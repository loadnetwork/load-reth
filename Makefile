.DEFAULT_GOAL := help

# Build configuration
PROFILE ?= release
# Performance features: jemalloc + asm-keccak (forwarded to reth)
FEATURES ?= jemalloc,asm-keccak

# Docker configuration (DockerHub)
DOCKER_IMAGE_NAME ?= loadnetwork/load-reth
DOCKER_BUILDKIT ?= 1
BUILDKIT_PROGRESS ?= plain
export DOCKER_BUILDKIT
export BUILDKIT_PROGRESS
DOCKER_BUILDX_PUSH ?= true
DOCKER_BUILDX_OUTPUT ?= type=oci,dest=dist/load-reth-multiarch.tar

# Git metadata
GIT_TAG := $(shell (git describe --exact-match --tags 2>/dev/null) || echo sha-$$(git rev-parse --short=12 HEAD))
GIT_SHA := $(shell git rev-parse --short=12 HEAD 2>/dev/null || echo "unknown")
AUDIT_IGNORES ?= RUSTSEC-2025-0055 RUSTSEC-2024-0388 RUSTSEC-2024-0436

AUDIT_FLAGS := --deny warnings
ifneq ($(strip $(AUDIT_IGNORES)),)
AUDIT_FLAGS += $(foreach advisory,$(AUDIT_IGNORES),--ignore $(advisory))
endif

##@ Help

.PHONY: help
help: ## Display this help
	@awk 'BEGIN {FS = ":.*##"; printf "Usage:\n  make \033[36m<target>\033[0m\n"} /^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-25s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)

##@ Build

.PHONY: build
build: ## Build the load-reth binary
	cargo build --bin load-reth --features "$(FEATURES)" --profile "$(PROFILE)" --locked

.PHONY: maxperf
maxperf: ## Build load-reth with maximum performance optimizations
	RUSTFLAGS="-C target-cpu=native" cargo build --bin load-reth --features "$(FEATURES)" --profile maxperf --locked

.PHONY: build-reproducible
build-reproducible: ## Build load-reth reproducibly (x86_64-unknown-linux-gnu, release)
	@echo "Building reproducible load-reth binary (release, x86_64-unknown-linux-gnu)..."
	@SOURCE_DATE_EPOCH=$$(git log -1 --pretty=%ct); \
	RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static-libgcc -C link-arg=-Wl,--build-id=none -C metadata='' --remap-path-prefix $$PWD=." ; \
	CARGO_INCREMENTAL=0 ; \
	LC_ALL=C ; \
	TZ=UTC ; \
	SOURCE_DATE_EPOCH=$$SOURCE_DATE_EPOCH RUSTFLAGS="$$RUSTFLAGS" CARGO_INCREMENTAL=$$CARGO_INCREMENTAL LC_ALL=$$LC_ALL TZ=$$TZ \
		cargo build --bin load-reth --features "$(FEATURES)" --profile release --locked --target x86_64-unknown-linux-gnu

# Cross-compilation targets (requires `cross` tool: cargo install cross)
.PHONY: build-x86_64-unknown-linux-gnu
build-x86_64-unknown-linux-gnu: ## Cross-compile for x86_64 Linux
	cross build --bin load-reth --target x86_64-unknown-linux-gnu --features "$(FEATURES)" --profile "$(PROFILE)" --locked

.PHONY: build-aarch64-unknown-linux-gnu
build-aarch64-unknown-linux-gnu: ## Cross-compile for aarch64 Linux
	JEMALLOC_SYS_WITH_LG_PAGE=16 cross build --bin load-reth --target aarch64-unknown-linux-gnu --features "$(FEATURES)" --profile "$(PROFILE)" --locked

##@ Test

.PHONY: test
test: ## Run the test suite
	cargo test --tests

.PHONY: test-nextest
test-nextest: ## Run the test suite with cargo-nextest (requires cargo-nextest)
	cargo install cargo-nextest --locked --quiet || true
	cargo nextest run --workspace --all-features

.PHONY: test-all
test-all: ## Run all tests including ignored
	cargo test --tests -- --include-ignored

##@ Coverage

.PHONY: cov
cov: ## Generate lcov.info via cargo-llvm-cov (requires cargo-llvm-cov)
	rm -f lcov.info
	cargo llvm-cov nextest --lcov --output-path lcov.info --workspace --all-features

.PHONY: cov-report-html
cov-report-html: cov ## Generate an HTML coverage report via cargo-llvm-cov
	cargo llvm-cov report --html

##@ Docker

# Note: Multi-platform builds use cross + Dockerfile.cross pattern (matches ultramarine).
# Requires: cargo install cross, docker buildx

.PHONY: docker-build-local
docker-build-local: ## Build a Docker image for local use (no push)
	docker build --tag $(DOCKER_IMAGE_NAME):local \
		--build-arg COMMIT=$(GIT_SHA) \
		--build-arg VERSION=$(GIT_TAG) \
		--build-arg BUILD_PROFILE=$(PROFILE) \
		--build-arg FEATURES="$(FEATURES)" \
		.

.PHONY: docker-build-debug
docker-build-debug: ## Fast debug build using Docker multistage (no cross-compilation)
	@echo "Building debug Docker image with in-container compilation..."
	docker build --file Dockerfile.debug --tag $(DOCKER_IMAGE_NAME):debug \
		--build-arg BUILD_PROFILE=$(PROFILE) \
		--build-arg FEATURES="$(FEATURES)" \
		.

.PHONY: docker-build-push-latest
docker-build-push-latest: ## Build cross-arch binaries and push multi-arch Docker image
	$(MAKE) build-x86_64-unknown-linux-gnu
	$(MAKE) build-aarch64-unknown-linux-gnu
	mkdir -p dist/bin/amd64 dist/bin/arm64
	cp target/x86_64-unknown-linux-gnu/$(PROFILE)/load-reth dist/bin/amd64/
	cp target/aarch64-unknown-linux-gnu/$(PROFILE)/load-reth dist/bin/arm64/
	docker buildx build --file ./Dockerfile.cross . \
		--platform linux/amd64,linux/arm64 \
		--tag $(DOCKER_IMAGE_NAME):$(GIT_TAG) \
		--tag $(DOCKER_IMAGE_NAME):latest \
		--provenance=false \
		$(if $(filter true,$(DOCKER_BUILDX_PUSH)),--push,--output=$(DOCKER_BUILDX_OUTPUT))

.PHONY: docker-run
docker-run: docker-build-local ## Build and run the Docker image locally (interactive shell)
	@echo "Starting load-reth container with shell access..."
	@echo "To run a node: /usr/local/bin/load-reth node --chain <chain> --datadir /data"
	docker run --rm -it \
		-p 30303:30303 -p 30303:30303/udp \
		-p 8545:8545 -p 8546:8546 -p 8551:8551 -p 9001:9001 \
		-v load-reth-data:/data \
		--entrypoint /bin/bash \
		$(DOCKER_IMAGE_NAME):local

.PHONY: docker-run-node
docker-run-node: docker-build-local ## Build and run load-reth node (requires CHAIN=<chainspec>)
ifndef CHAIN
	$(error CHAIN is required. Usage: make docker-run-node CHAIN=path/to/chain.json)
endif
	docker run --rm -it \
		-p 30303:30303 -p 30303:30303/udp \
		-p 8545:8545 -p 8546:8546 -p 8551:8551 -p 9001:9001 \
		-v load-reth-data:/data \
		-v $(CHAIN):/chain.json:ro \
		$(DOCKER_IMAGE_NAME):local node --chain /chain.json --datadir /data --http --http.addr 0.0.0.0

##@ Quality

.PHONY: fmt
fmt: ## Format code with nightly rustfmt
	cargo +nightly fmt --all

.PHONY: fmt-check
fmt-check: ## Check code formatting
	cargo +nightly fmt --all --check

.PHONY: clippy
clippy: ## Run clippy lints
	cargo clippy --all-targets --all-features

.PHONY: sort
sort: ## Sort dependencies in Cargo.toml
	cargo sort --workspace

.PHONY: sort-check
sort-check: ## Check if dependencies are sorted
	cargo sort --workspace --check

.PHONY: docs
docs: ## Build documentation with warnings as errors
	RUSTDOCFLAGS="-D warnings" cargo doc --all --no-deps --document-private-items

.PHONY: lint-typos
lint-typos: ensure-typos ## Run typos CLI across the repo
	typos

.PHONY: ensure-typos
ensure-typos:
	@if ! command -v typos >/dev/null 2>&1; then \
		echo "typos not found. Install it with \`cargo install typos-cli\`."; exit 1; \
	fi

.PHONY: deny
deny: ## Run cargo-deny checks
	cargo deny check

.PHONY: audit
audit: ## Run security audit
	cargo audit $(AUDIT_FLAGS)

##@ CI

.PHONY: ci
ci: lint test docs deny audit ## Run all CI checks locally

.PHONY: lint
lint: fmt-check clippy sort-check lint-typos ## Run all linters

.PHONY: pr
pr: lint test docs deny audit ## Run all checks before PR - use this!

.PHONY: pr-fix
pr-fix: fmt sort ## Auto-fix formatting and sorting issues

.PHONY: ci-tools
ci-tools: ## Install CI tools
	cargo install cargo-deny cargo-audit cargo-sort --locked

.PHONY: clean
clean: ## Clean build artifacts
	cargo clean
	rm -rf dist/
