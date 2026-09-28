# sandy

Sandy runs Python and JavaScript locally, in Docker, or in a Firecracker microVM, and it can sandbox shell commands.

## Requirements

- **Rust** (2021 edition) to build from source
- **Local mode**: [`uv`](https://docs.astral.sh/uv/) on `PATH` for Python; [Node.js](https://nodejs.org/) (`node`) on `PATH` for JavaScript (`npm` only for `// /// script` dependencies)
- **Docker mode** (optional `docker` feature): a Docker or Colima daemon
- **Bubblewrap shell sandbox** (optional): `bwrap` on `PATH` (Linux). Ubuntu 24.04+ also needs an AppArmor `userns` profile for `/usr/bin/bwrap`. Without it, `bwrap` fails with `setting up uid map: Permission denied`. Install `contrib/apparmor/bwrap`:

  ```bash
  sudo cp contrib/apparmor/bwrap /etc/apparmor.d/bwrap
  sudo apparmor_parser -r /etc/apparmor.d/bwrap
  ```
- **Firecracker mode** (default on Linux): `/dev/kvm`, the `firecracker` binary, a kernel, and a Sandy rootfs; see [Firecracker](docs/firecracker.md)
- **Python extension**: Python 3.12+ and [`maturin`](https://www.maturin.rs/) when you build the `sandy` wheel

## Install and quick start

Clone the repo and build from the repository root:

```bash
git clone https://github.com/Bioto/sandy.git
cd sandy
cargo build --release
```

The binary is named **`sandy`**. Run `sandy --help` for subcommands.

### Tests

Run `make test` (Rust + Python), or `make test-rust` / `make test-python` alone. See [Development](docs/development.md#tests).

### Local: run inline Python

```bash
cargo run -- exec-code --code 'print("hello")'
```

Or pipe code on stdin:

```bash
echo 'print(1 + 1)' | cargo run -- exec-code
```

### Local: run a Python script

```bash
cargo run -- py03 ./my_script.py
```

### Local: run inline JavaScript

```bash
cargo run -- exec-js --code 'console.log("hello")'
```

Or pipe code on stdin:

```bash
echo 'console.log(1 + 1)' | cargo run -- exec-js
```

Inline npm dependencies use a `// /// script` block (same TOML shape as Python PEP 723):

```javascript
// /// script
// dependencies = ["ms"]
// ///
import ms from "ms";
console.log(ms("2 days"));
```

### Local: run a JavaScript file

```bash
cargo run -- js ./my_script.mjs
```

### Docker: build the image

Build context is the repository root (this directory contains `Cargo.toml` and `.docker/`):

```bash
docker build -f .docker/Dockerfile -t sandy:latest .
```

Optional: set the Python version at build time with `--build-arg PYTHON_VERSION=3.12`.

### Docker: run Python or JavaScript in a container

If **`sandy:latest`** is missing, `docker-exec` runs **`docker build`** automatically. Pass **`--no-auto-build`** to fail instead.

**Network:** Default is **no outbound** (`network_mode=none`). Add **`--allow-network`** when you need PyPI or npm downloads:

```bash
# Isolated (default)
cargo run --features docker -- docker-exec --code 'print("from container")'

# Outbound allowed (PyPI, etc.)
cargo run --features docker -- docker-exec --allow-network --code 'print("from container")'

# JavaScript (same XML output; --allow-network for npm installs)
cargo run --features docker -- docker-exec-js --code 'console.log("from container")'
```

### Firecracker: run Python or JavaScript in a microVM

Build the kernel and rootfs with the [Firecracker guide](docs/firecracker.md), then run:

```bash
cargo run --features firecracker -- fc-exec \
  --kernel .firecracker/out/vmlinux \
  --rootfs .firecracker/out/rootfs.ext4 \
  --code 'print("from microVM")'
```

Manage a persistent VM with `sandy microvm start`, `sandy microvm status`, and `sandy microvm stop`.

Output is XML that describes stdout, stderr, and metrics.

### Interactive shell

```bash
cargo run -- shell
```

## Features

- **Local execution**: Run Python through `uv run` or JavaScript through Node.js, with streaming stdout and captured exit codes.
- **Docker execution**: Run the same CLI inside a container, with structured **XML** output (stdout, stderr, timing, exit code).
- **Firecracker execution**: Run Python and JavaScript inside a Linux microVM with a separate guest kernel.
- **Library API**: Embed `PythonExecutionService` / `JsExecutionService` (always) and `DockerService` (with `docker` feature), plus `sandy::exec_sandbox` for shell isolation.
- **Python bindings (PyO3 + maturin)**: The **`sandy` package** (`python/sandy/`) wraps the native module **`sandy._native`**. Build with `maturin develop` / `maturin build` from the repo root (see `pyproject.toml`).
- **Pluggable shell sandbox (`exec_sandbox`)**: Embed `NoneSandbox`, `BubblewrapSandbox` (Linux + `bwrap`), or optional `DockerShellSandbox`.
- **Desktop JavaScript bindings**: `sandy-js/` provides thin napi-rs bindings for Node.js and Bun.
- **Flexible Docker connectivity**: Honors `DOCKER_HOST`, then Colima sockets, then local defaults (when `docker` is enabled).

## Cargo features

Docker is **optional**. You can use sandy for local Python and shell sandboxing without linking bollard.

| Feature | Default | Purpose |
|---------|---------|---------|
| `python-bindings` | yes | PyO3 extension (`sandy._native`) |
| `exec-sandbox` | no | Marker for embedders; `exec_sandbox` is always in the library |
| `docker` | no | `DockerService`, `docker-exec` / `docker-exec-js` CLI, `DockerShellSandbox`, Python Docker bindings |
| `firecracker` | yes | Firecracker microVM service, `fc-exec` / `fc-exec-js`, and `FirecrackerShellSandbox` |

| Build | Command |
|-------|---------|
| Local Python + shell sandbox only | `cargo build --no-default-features` |
| Full CLI (local + Docker) | `cargo build --features docker` |
| Embedder without defaults | `sandy = { default-features = false, features = ["exec-sandbox"] }` |
| Python wheel with Docker | `maturin develop --features python-bindings,docker` |

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/architecture.md) | Layout, execution modes, data flow |
| [CLI reference](docs/cli-reference.md) | All commands and flags |
| [Docker guide](docs/docker.md) | Building images, config, XML output, timeouts |
| [Library API](docs/api.md) | Types and public methods |
| [Development](docs/development.md) | Build, test, logging |
| [Firecracker](docs/firecracker.md) | MicroVM kernel, rootfs, and CLI |

## `sandy-js`

Build the desktop binding from `sandy-js/`:

```bash
cd sandy-js
npm ci
npm test
npm run build:native
```

`runShell` requires an absolute workspace directory. It uses a clean environment, **denies network access by default**, limits command time to 600 seconds, and bounds each output stream. It rejects the unsandboxed backend and fails when no selected backend is available.

Android does not load this package. Harn Android uses a remote-only TUI profile, so it does not mount a local directory or execute local shell commands.

## Python package

From the repository root (after `sandy:latest` exists if you use `no_auto_build=True`):

```bash
uvx maturin develop --release
```

```python
from sandy import execute_python_docker

stdout, stderr, exit_code = execute_python_docker(
    'print("hello")',
    allow_network=False,
    no_auto_build=True,
    python_version="3.13",
)
```

### Optional: global dependencies and bootstrap

For embedders that want the same [PEP 723](https://docs.astral.sh/uv/guides/scripts/) packages or shared Python on **every** execution:

- **`global_dependencies`**: list of dependency spec strings. Merged into the script’s `# /// script` block.
- **`bootstrap_python`**: source inserted right after the closing `# ///` of the PEP 723 block, or at the start of the file if there is no block.

```python
from sandy import execute_python_docker, prepare_script

stdout, stderr, code = execute_python_docker(
    user_script,
    global_dependencies=["httpx"],
    bootstrap_python="def shared(): return 1\n",
)
```

**Security:** `global_dependencies` and `bootstrap_python` are **trusted caller** input. They execute in the same sandbox as the user script. Pass only values you control, not untrusted user input.

**Layout note:** The native symbol lives in `sandy._native`. Import from `sandy` for wrappers that apply `prepare_script` / `prepare_js_script`.

## License

This project is licensed under **BSD-4-Clause**.

Public materials that mention this software must display the acknowledgement in the LICENSE file.

This license is GPL-incompatible.

See [LICENSE](LICENSE) for the full text.
