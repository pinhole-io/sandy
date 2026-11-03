# Development

## Requirements

- Rust with the 2021 edition
- `uv` on `PATH`, for local Python and for `make test-python`
- `node` on `PATH`, for local JavaScript. `npm` is required for inline JavaScript dependencies.
- Docker or Colima, for Docker tests and for `docker-exec`
- The Docker image build uses the nightly Rust image inside `.docker/Dockerfile`. A local `cargo build` uses your stable toolchain.

## Build

From this repository:

```bash
cargo build
cargo build --release
cargo run -- --help
cargo run -- exec-code --code 'print(1)'
cargo run -- exec-js --code 'console.log(1)'
```

## Tests

`make test` runs `cargo test` and `uv run --extra dev pytest`.

| Target | Command |
| --- | --- |
| Rust and Python | `make test` |
| Rust only | `make test-rust` |
| Python only | `make test-python` |

`CARGO_TEST_FLAGS` and `PYTEST_ARGS` are forwarded.

```bash
make test-rust CARGO_TEST_FLAGS="--lib"
make test-python PYTEST_ARGS="tests/test_script_prep.py"
```

### Docker integration tests

`tests/docker_integration.rs` is marked `#[ignore]`. Those tests need a Docker daemon and a usable image.

```bash
cargo test --features docker --test docker_integration -- --ignored
```

### Firecracker integration tests

`tests/firecracker_integration.rs` is marked `#[ignore]`. Those tests need Linux KVM and the guest artifacts.

```bash
cargo test --features firecracker --test firecracker_integration -- --ignored
```

## Logging

The binary starts `tracing-subscriber` with an `EnvFilter`.

- A set `RUST_LOG` value is the filter.
- An unset `RUST_LOG` uses `info`.

```bash
RUST_LOG=debug cargo run -- exec-code --code 'print(1)'
RUST_LOG=sandy=trace cargo run -- exec-code --code 'print(1)'
```

## Format and lint

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
```

## Node.js binding

From `sandy-js/`:

```bash
npm ci
npm test
```

## Python package

From this repository:

```bash
uvx maturin develop --release
uv run --extra dev pytest
```

The package source is `python/sandy/`. The extension module is `sandy._native`.

## Related

- [Docker guide](docker.md)
- [Firecracker guide](firecracker.md)
- [CLI reference](cli-reference.md)
