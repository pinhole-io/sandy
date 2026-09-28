# Development

## Prerequisites

- **Rust** with the 2021 edition (toolchain as pinned by your repo or `rustup`)
- **`uv`** on `PATH` for local `PythonExecutionService` tests and runs
- **`node`** on `PATH` for local `JsExecutionService` tests and runs (`npm` if you exercise inline JS dependencies)
- **Docker** (or **Colima**) for Docker-backed tests and manual `docker-exec` checks
- The **nightly** toolchain may be required if you build using the provided Dockerfile (see [Docker guide](docker.md#building-the-image))

## Clone and build

From the **`sandy/`** crate directory:

```bash
cd sandy
cargo build
cargo build --release
```

If this crate is a member of a Cargo workspace, run the same targets with `-p sandy` from the **workspace root**.

## Run the binary

```bash
cd sandy
cargo run -- --help
cargo run -- exec-code --code 'print(1)'
cargo run -- exec-js --code 'console.log(1)'
```

## Tests

From **`sandy/`**, **`make test`** runs **`cargo test`** and **`uv run --extra dev pytest`**. Use **`make test-rust`** or **`make test-python`** for only one stack; optional **`CARGO_TEST_FLAGS`** / **`PYTEST_ARGS`** are forwarded.

### Unit tests

```bash
cd sandy
cargo test
```

Includes tests such as `DockerConfig::validate`, XML escaping in `docker_exec`, etc.

### Integration tests (Docker)

`tests/docker_integration.rs` contains tests marked **`#[ignore]`** because they need a live Docker daemon (and a suitable image tag).

Run ignored tests explicitly:

```bash
cd sandy
cargo test -- --ignored
```

## Logging

The binary initializes `tracing-subscriber` with an **`EnvFilter`**:

- If **`RUST_LOG`** is set, that filter is used.
- Otherwise defaults to **`info`**.

Examples:

```bash
cd sandy
RUST_LOG=debug cargo run -- docker-exec --code 'print(1)'
RUST_LOG=sandy=trace cargo run -- exec-code --code 'print(1)'
```

## Rename status (`nexus_sandbox` → `sandy`)

| Area | Status |
|------|--------|
| `Cargo.toml` `name = "sandy"` | Crate and default binary name |
| `src/cli/mod.rs` Clap `name = "sandy"` | Program name in `--help` |
| `.docker/Dockerfile` | Builds **`sandy`** musl binary; `ENTRYPOINT` is **`sandy`** |
| `.docker/build.sh` | Defaults **`IMAGE_NAME=sandy`**, `sandy/.docker/Dockerfile` from workspace layout |
| `tests/docker_integration.rs` | Uses `sandy::...` |

Older docs or comments may still mention **`nexus_sandbox`** / **`nexus-py`**; treat them as legacy names.

## Formatting and lints

Use project-standard `cargo fmt` and `cargo clippy` if your repo enforces them:

```bash
cd sandy
cargo fmt
cargo clippy -- -D warnings
```

## Related

- [Docker guide](docker.md)
- [CLI reference](cli-reference.md)
