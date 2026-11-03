# Sandy

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust edition 2021](https://img.shields.io/badge/rust-edition%202021-orange.svg)](Cargo.toml)
[![Python 3.12+](https://img.shields.io/badge/python-3.12%2B-blue.svg)](pyproject.toml)
[![Node.js 18+](https://img.shields.io/badge/node.js-18%2B-green.svg)](sandy-js)

**Sandboxed Python, JavaScript, and shell execution.**

Sandy runs code on the host, in Docker, or in a Firecracker microVM. The Rust library, the `sandy` CLI, the Python package, and the Node.js binding share one execution policy.

[Quick start](#quick-start) · [Install](#install) · [CLI](docs/cli-reference.md) · [Docker](docs/docker.md) · [Firecracker](docs/firecracker.md) · [API](docs/api.md)

## Contents

- [Requirements](#requirements)
- [Install](#install)
- [Quick start](#quick-start)
- [Features](#features)
- [Cargo features](#cargo-features)
- [How it fits together](#how-it-fits-together)
- [Language bindings](#language-bindings)
- [Documentation](#documentation)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [License](#license)

## Requirements

- Rust 2021 edition, to build from source.
- [`uv`](https://docs.astral.sh/uv/) on `PATH`, for local Python.
- [Node.js](https://nodejs.org/) 18 or newer (`node` on `PATH`), for local JavaScript. `npm` is required only for `// /// script` dependencies.
- A Docker or Colima daemon, for the optional `docker` feature.
- Linux, `/dev/kvm`, and the Firecracker artifacts, for microVM mode. See [Firecracker](docs/firecracker.md).
- `bwrap` on Linux, for the bubblewrap shell sandbox.
- Python 3.12 or newer and [`maturin`](https://www.maturin.rs/), to build the Python extension.

### Bubblewrap on Ubuntu 24.04 or newer

`bwrap` fails with `setting up uid map: Permission denied` when AppArmor blocks user namespaces.

1. Copy the profile:

```bash
sudo cp contrib/apparmor/bwrap /etc/apparmor.d/bwrap
sudo apparmor_parser -r /etc/apparmor.d/bwrap
```

2. Run the command again.

## Install

There is no crates.io, PyPI, or npm package. Build from this repository.

| Surface | Where it runs | How to build |
| --- | --- | --- |
| Rust library and CLI | This repository | `cargo build --release` |
| Python | CPython 3.12 or newer | `maturin` from this repository |
| JavaScript | Node.js 18 or newer, and Bun | `sandy-js/` |

### Rust

```bash
git clone https://github.com/pinhole-io/sandy.git
cd sandy
cargo build --release
```

The binary name is `sandy`. Run `sandy --help`.

Add the library from git:

```toml
[dependencies]
sandy = { git = "https://github.com/pinhole-io/sandy" }
```

Pin a commit with `rev`. Enable a feature in the same dependency:

```toml
sandy = { git = "https://github.com/pinhole-io/sandy", default-features = false, features = ["exec-sandbox"] }
```

### Python

The import name is `sandy`. The package source is [`python/sandy/`](python/sandy/).

```bash
uvx maturin develop --release
```

The default build includes Python bindings and Firecracker. Add Docker with:

```bash
uvx maturin develop --release --features python-bindings,docker
```

### JavaScript

Build the binding from `sandy-js/`. Details: [sandy-js/README.md](sandy-js/README.md).

```bash
cd sandy-js
npm ci
npm test
```

## Quick start

### Local Python

```bash
cargo run -- exec-code --code 'print("hello")'
echo 'print(1 + 1)' | cargo run -- exec-code
cargo run -- py03 ./my_script.py
```

A script can declare [PEP 723](https://docs.astral.sh/uv/guides/scripts/) dependencies:

```python
# /// script
# dependencies = ["httpx"]
# ///
import httpx
print(httpx.get("https://example.com", timeout=10).status_code)
```

### Local JavaScript

```bash
cargo run -- exec-js --code 'console.log("hello")'
echo 'console.log(1 + 1)' | cargo run -- exec-js
cargo run -- js ./my_script.mjs
```

Inline npm dependencies use a `// /// script` block. The TOML shape matches PEP 723:

```javascript
// /// script
// dependencies = ["ms"]
// ///
import ms from "ms";
console.log(ms("2 days"));
```

### Docker

Build the image from this repository:

```bash
docker build -f .docker/Dockerfile -t sandy:latest .
```

Set the Python version with `--build-arg PYTHON_VERSION=3.12`.

`docker-exec` builds `sandy:latest` when the image is missing. Pass `--no-auto-build` to fail instead. The default network mode is `none`. Pass `--allow-network` when the script must reach PyPI or npm.

```bash
cargo run --features docker -- docker-exec --code 'print("from container")'
cargo run --features docker -- docker-exec --allow-network --code 'print("from container")'
cargo run --features docker -- docker-exec-js --code 'console.log("from container")'
```

The command prints XML. The document contains stdout, stderr, timing, and the exit code.

### Firecracker

Build the kernel and the rootfs with the [Firecracker guide](docs/firecracker.md). Then run:

```bash
cargo run --features firecracker -- fc-exec \
  --kernel .firecracker/out/vmlinux \
  --rootfs .firecracker/out/rootfs.ext4 \
  --code 'print("from microVM")'
```

`sandy microvm start`, `sandy microvm status`, and `sandy microvm stop` manage a persistent VM. The output is the same XML document as `docker-exec`.

### Interactive shell

```bash
cargo run -- shell
```

## Features

| Feature | What you get |
| --- | --- |
| Local Python | `uv run`, streaming stdout, and the child exit code |
| Local JavaScript | `node`, optional `npm install --ignore-scripts`, and the child exit code |
| Docker | A container per run, or a persistent container. XML output. |
| Firecracker | A Linux microVM with a separate guest kernel. XML output. |
| Shell sandbox | `NoneSandbox`, `BubblewrapSandbox`, and optional Docker or Firecracker shells |
| Python package | `import sandy` over the native extension |
| Node.js binding | `sandy-js` for desktop Node.js and Bun |

## Cargo features

Docker is optional. Local Python and the shell sandbox do not link bollard.

| Feature | Default | Purpose |
| --- | --- | --- |
| `python-bindings` | yes | PyO3 extension `sandy._native` |
| `firecracker` | yes | MicroVM service, `fc-exec`, `fc-exec-js`, and `microvm` |
| `docker` | no | `DockerService`, `docker-exec`, `docker-exec-js`, and `DockerShellSandbox` |
| `exec-sandbox` | no | Marker for embedders. The library always includes `exec_sandbox`. |

| Build | Command |
| --- | --- |
| Local Python and the shell sandbox | `cargo build --no-default-features` |
| CLI with Docker | `cargo build --features docker` |
| Embedder without default features | `sandy = { default-features = false, features = ["exec-sandbox"] }` |
| Python wheel with Docker | `maturin develop --features python-bindings,docker` |

## How it fits together

```text
CLI / Python / Node.js / Rust embedder
            │
            ▼
          sandy
            │
            ├── uv / node          local execution
            ├── bollard            Docker, optional
            ├── Firecracker        microVM, Linux
            └── exec_sandbox       shell isolation
```

- The Rust crate owns process limits, backend selection, and the CLI.
- Docker and Firecracker run the same `sandy` binary inside the guest.
- `sandy-js` calls `exec_sandbox`. It does not copy the policy into JavaScript.
- The Python package prepares a script, then calls `sandy._native`.

Read the map in [Architecture](docs/architecture.md).

## Language bindings

### Python

**Warning:** `global_dependencies` and `bootstrap_python` are trusted caller input. They run in the same sandbox as the user script. Pass only values you control.

```python
from sandy import execute_python_docker

stdout, stderr, exit_code = execute_python_docker(
    'print("hello")',
    allow_network=False,
    no_auto_build=True,
    python_version="3.13",
)
```

`allow_network` defaults to `True` in the Python helpers. The CLI default is no egress. Pass `allow_network=False` when you want the CLI default.

`no_auto_build=True` is the Python default. Build `sandy:latest` before that call, or pass `no_auto_build=False`.

Package notes: [python/README.md](python/README.md).

### JavaScript

`runShell` requires an absolute workspace directory. It uses a clean environment. Network access is off unless `allowNetwork` is true. The default timeout is 120 seconds. The maximum timeout is 600 seconds. Each output stream is bounded. The call rejects the unsandboxed backend. It fails when no selected backend is available.

Android does not load this package.

Package notes: [sandy-js/README.md](sandy-js/README.md).

## Documentation

| Goal | Start here |
| --- | --- |
| See the layout and the data flow | [Architecture](docs/architecture.md) |
| Run a command or read a flag | [CLI reference](docs/cli-reference.md) |
| Build the image and read the XML | [Docker](docs/docker.md) |
| Boot a microVM | [Firecracker](docs/firecracker.md) |
| Call the Rust types | [Library API](docs/api.md) |
| Build, test, and set logs | [Development](docs/development.md) |
| Open every guide | [Documentation index](docs/README.md) |

## Troubleshooting

| What you see | What to do |
| --- | --- |
| `uv` not found | Install `uv` and put it on `PATH`. |
| `node` not found | Install Node.js 18 or newer. |
| `bwrap` reports `setting up uid map: Permission denied` | Install the AppArmor profile in [Requirements](#requirements). |
| Cannot connect to Docker | Start the daemon. Colima uses its own socket. Sandy checks `DOCKER_HOST`, then the Colima sockets, then the local default. |
| `docker-exec` cannot find the Dockerfile | Run it from this repository, or pass `--dockerfile .docker/Dockerfile`. |
| The container cannot install a package | Pass `--allow-network`. The default network mode is `none`. |
| `fc-exec` cannot open `/dev/kvm` | Use a Linux host with KVM. Build the kernel and the rootfs first. |
| Python cannot import `sandy` | Run `uvx maturin develop --release` in this repository. |
| `execute_python_docker` raises a runtime error | Rebuild with `--features python-bindings,docker`. |
| Node cannot load `sandy-js` | Run `npm run build:native` in `sandy-js/`. |

## Development

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
make test
```

`make test` runs `cargo test` and `uv run --extra dev pytest`. Use `make test-rust` or `make test-python` for one stack.

`RUST_LOG` selects the tracing filter. The default level is `info`.

```bash
RUST_LOG=debug cargo run -- exec-code --code 'print(1)'
```

More detail: [Development](docs/development.md).

## License

The license is [MIT](LICENSE).

Keep the copyright notice and the permission notice in every copy.
