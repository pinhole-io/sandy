# sandy (Python)

Python bindings for the Sandy Rust crate.

The import name is `sandy`. The package source is this directory. The native extension is `sandy._native`.

## Requirements

- Python 3.12 or newer
- A Rust toolchain
- [`uv`](https://docs.astral.sh/uv/) on `PATH` when you run local Python through the CLI
- A Docker daemon when you call the Docker helpers
- Linux KVM and Firecracker artifacts when you call the Firecracker helpers

## Build

From the repository root:

```bash
uvx maturin develop --release
```

That command enables `python-bindings`. Firecracker stays on because it is a default Cargo feature. Docker stays off.

Add Docker:

```bash
uvx maturin develop --release --features python-bindings,docker
```

## Run Python or JavaScript

**Warning:** `global_dependencies`, `bootstrap_python`, and `bootstrap_javascript` are trusted caller input. They run in the same sandbox as the user script. Pass only values you control.

```python
from sandy import execute_python_docker, prepare_script

stdout, stderr, exit_code = execute_python_docker(
    'print("hello")',
    allow_network=False,
    no_auto_build=True,
)
```

| Helper | Requirement | What it does |
| --- | --- | --- |
| `prepare_script` | none | Merges PEP 723 dependencies and inserts trusted Python |
| `prepare_js_script` | none | Merges `// /// script` dependencies and inserts trusted JavaScript |
| `execute_python_docker` | `docker` feature and a Docker daemon | Runs Python in the `sandy` image |
| `execute_javascript_docker` | `docker` feature and a Docker daemon | Runs JavaScript in the `sandy` image |
| `execute_python_firecracker` | `firecracker` feature, Linux, and KVM | Runs Python in a microVM |
| `execute_javascript_firecracker` | `firecracker` feature, Linux, and KVM | Runs JavaScript in a microVM |

Each execute helper returns `(stdout, stderr, exit_code)`.

`allow_network` defaults to `True` for the Docker helpers and `False` for the Firecracker helpers. The CLI default for Docker is no egress. Pass `allow_network=False` when you want that CLI default.

`no_auto_build` defaults to `True`. Build `sandy:latest` first, or pass `no_auto_build=False`.

Image build steps: [Docker guide](../docs/docker.md).

## Tests

From the repository root:

```bash
uv run --extra dev pytest
```

`tests/test_script_prep.py` covers script preparation. It does not start Docker or Firecracker.

## License

The license is [MIT](../LICENSE).
