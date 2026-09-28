# CLI reference

The binary is produced from the **`sandy`** package (`cargo build -p sandy` or `cargo install --path .`). Clap uses the program name **`sandy`**.

```bash
sandy --help
sandy <subcommand> --help
```

## Global behavior

- **Logging**: [`tracing`](https://docs.rs/tracing) with `tracing-subscriber`. Set `RUST_LOG` (e.g. `RUST_LOG=debug`) for verbose logs. Default filter is `info` if `RUST_LOG` is unset.
- **Exit codes**: Subcommands that run Python or JavaScript propagate the **child exit code** on failure (`py03`, `exec-code`, `js`, `exec-js`, `docker-exec`, `docker-exec-js`). Success is exit `0`.

---

## `shell`

Interactive REPL-style loop: prints a `>>>` prompt, reads stdin line by line.

### Flags

| Flag | Description |
|------|-------------|
| `-v`, `--verbose` | Print extra debug lines for non-help input |

### Built-in inputs

| Input | Behavior |
|-------|----------|
| `help` | Show built-in help |
| `exit` / `quit` | Exit the shell |
| *(anything else)* | Echoed to stdout (with optional verbose debug) |

### Example

```bash
sandy shell
sandy shell --verbose
```

---

## `py03`

Runs a Python **file** through **`uv run`** with **streaming stdout** (each line printed as it arrives).

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<script>` | Yes | Path to the `.py` script |

### Behavior

- Uses `PythonExecutionService::execute_script_streaming`.
- If stderr is non-empty after completion, it is printed to **stderr**.
- Non-zero script exit code causes the process to exit with that code.

### Example

```bash
sandy py03 ./tools/my_task.py
```

---

## `js`

Runs a JavaScript **file** through **`node`** with **streaming stdout**. If the file has a leading `// /// script` block with `dependencies`, sandy installs those npm specs (`npm install --ignore-scripts`) in a temp workspace first.

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<script>` | Yes | Path to the `.js` / `.mjs` script |

### Behavior

- Uses `JsExecutionService::execute_script_streaming`.
- If stderr is non-empty after completion, it is printed to **stderr**.
- Non-zero script exit code causes the process to exit with that code.

### Example

```bash
sandy js ./tools/my_task.mjs
```

---

## `exec-code`

Runs Python **source** via **`uv run`**: either from `--code` or from **stdin**.

### Flags

| Flag | Description |
|------|-------------|
| `-c`, `--code <CODE>` | Python source to execute |

If `--code` is omitted, the entire **stdin** is read as the script body.

### Errors

- Empty code (after trim) → message on stderr and exit `1`.

### Examples

```bash
sandy exec-code --code 'print("hi")'
echo 'print(2 + 2)' | sandy exec-code
```

---

## `exec-js`

Runs JavaScript **source** via **`node`**: either from `--code` or from **stdin**. Inline `// /// script` dependencies are installed with npm the same way as `js`.

### Flags

| Flag | Description |
|------|-------------|
| `-c`, `--code <CODE>` | JavaScript source to execute |

If `--code` is omitted, the entire **stdin** is read as the script body.

### Errors

- Empty code (after trim) → message on stderr and exit `1`.

### Examples

```bash
sandy exec-js --code 'console.log("hi")'
echo 'console.log(2 + 2)' | sandy exec-js
```

---

## `docker-exec`

Connects to Docker, runs **`execute_python_code`** (new ephemeral container), and prints results as **XML** to stdout. **Async** (requires Tokio runtime — satisfied by the binary).

### Flags

| Flag | Default (if applicable) | Description |
|------|-------------------------|-------------|
| `-c`, `--code <CODE>` | — | Python source; if omitted, read from stdin |
| `--image` | `sandy` | Docker image repository/name |
| `--tag` | `latest` | Image tag |
| `--dockerfile` | auto-detect, else `.docker/Dockerfile` | Path to Dockerfile (first existing among `.docker/Dockerfile`, `sandy/.docker/Dockerfile`, `src/sandy/.docker/Dockerfile`) |
| `--build-context` | `.` | Build context path (stored in `DockerConfig`) |
| `--no-auto-build` | off | If set, do not run `docker build` when the image is missing |
| `--python-version` | `3.13` | `PYTHON_VERSION` build-arg when auto-building (pick a version uv provides for musl if using Alpine) |
| `--allow-network` | off | If set, container gets outbound network (PyPI, `uv` downloads). Default is **no** egress (`network_mode=none`) |

**Note:** If `image:tag` is missing, `docker-exec` runs **`docker build`** (unless `--no-auto-build`) using `--dockerfile`, `--build-context`, and `--python-version`. Manual build commands: [README – Docker: build the image](../README.md#docker-build-the-image) or [Docker guide – Building the image](docker.md#building-the-image).

**Network:** By default the sandbox uses **isolated** networking (no outbound). Use **`--allow-network`** when code or `uv` must reach the internet (inline dependencies, PyPI installs, Python runtime downloads).

### Output

See [Docker guide – XML output](docker.md#xml-output-from-docker-exec).

### Examples

```bash
sandy docker-exec --code 'print(1)'
echo 'print("stdin")' | sandy docker-exec --image sandy --tag latest
```

---

## `docker-exec-js`

Same as **`docker-exec`** (XML output, image auto-build, `--allow-network`) but runs **`execute_javascript_code`** (`exec-js --code` inside the container).

Use **`--allow-network`** when the script’s `// /// script` block needs the npm registry.

### Examples

```bash
sandy docker-exec-js --code 'console.log(1)'
echo 'console.log("stdin")' | sandy docker-exec-js
```

---

## Quick comparison

| Command | Backend | Typical use |
|---------|---------|----------------|
| `shell` | None (demo REPL) | Interactive testing |
| `py03` | Local `uv` | Run a Python script with streaming |
| `exec-code` | Local `uv` | One-off Python code string or piped script |
| `js` | Local `node` / `npm` | Run a JavaScript file with streaming |
| `exec-js` | Local `node` / `npm` | One-off JavaScript code string or piped script |
| `docker-exec` | Docker + `uv` in container | Isolated Python run, XML for agents/automation |
| `docker-exec-js` | Docker + `node` in container | Isolated JavaScript run, XML for agents/automation |
| `fc-exec` | Firecracker + `uv` in a microVM | KVM-isolated Python run, XML for agents/automation |
| `fc-exec-js` | Firecracker + `node` in a microVM | KVM-isolated JavaScript run, XML for agents/automation |

---

## `fc-exec` and `fc-exec-js`

These commands require the `firecracker` Cargo feature. They start one
Firecracker microVM, send the source to `fc-agent` over vsock, and print XML.

### Flags

| Flag | Description |
|------|-------------|
| `-c`, `--code <CODE>` | Source code; if omitted, read from stdin |
| `--kernel <PATH>` | Firecracker kernel image; defaults to `SANDY_FC_KERNEL` or `.firecracker/out/vmlinux` |
| `--rootfs <PATH>` | Guest ext4 rootfs; defaults to `SANDY_FC_ROOTFS` or `.firecracker/out/rootfs.ext4` |
| `--allow-network` | Attach the TAP device named by `SANDY_FC_TAP` |

The default VM has no network interface. The commands use the same XML
`<execution>` output as `docker-exec`.

---

## `microvm`

Manage persistent Firecracker processes. State is stored in
`SANDY_FC_STATE_DIR` or `~/.cache/sandy/firecracker/vms`.

```bash
sandy microvm start --name worker --kernel ./vmlinux --rootfs ./rootfs.ext4
sandy microvm status
sandy microvm status --name worker --json
sandy microvm stop --name worker
```

`microvm start` accepts `--binary`, `--vcpu-count`, `--memory-mib`,
`--tap-device`, and `--state-dir`. `microvm stop --force` sends `SIGKILL`.
