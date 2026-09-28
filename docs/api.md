# Library API

`sandy` is a library crate. Import from the **`sandy`** crate:

```rust
use sandy::{
    Cli, Commands,
    DockerConfig, DockerError, DockerService,
    ExecutionResult, PythonExecutionError, PythonExecutionService,
    JsExecutionError, JsExecutionService,
    FirecrackerConfig, FirecrackerError, FirecrackerService,
};
```

Public re-exports are defined in `src/lib.rs`.

---

## `ExecutionResult`

`src/service/mod.rs`

| Field | Type | Meaning |
|-------|------|---------|
| `stdout` | `String` | Collected standard output |
| `stderr` | `String` | Collected standard error |
| `exit_code` | `i32` | Process exit code (`-1` if missing) |

---

## `PythonExecutionError`

`#[derive(Error)]` enum in `src/service/mod.rs`:

| Variant | When |
|---------|------|
| `UvNotFound` | `uv` not found on `PATH` |
| `ScriptNotFound(String)` | Script path does not exist |
| `ExecutionFailed(String)` | Spawn/wait or other execution failure |
| `IoError(String)` | I/O or callback errors |

---

## `PythonExecutionService`

Stateless service type; construct with `PythonExecutionService::new()`.

| Method | Returns | Description |
|--------|---------|-------------|
| `execute_script(&Path)` | `Result<ExecutionResult, PythonExecutionError>` | `uv run <path>`; full stdout/stderr capture |
| `execute_code(&str)` | Same | Writes code to a temp `.py` file, then `execute_script` |
| `execute_script_streaming(&Path, on_output)` | Same | Per-line stdout callback; stderr lines logged with `tracing::error!` |
| `execute_code_streaming(&str, on_output)` | Same | Temp `.py` with `0o755` on Unix, then `execute_script_streaming` |

**Callback type** for streaming: `impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>`.

Scripts may use **uv inline dependency** comments as supported by uv.

---

## `JsExecutionError`

`#[derive(Error)]` enum in `src/service/js.rs`:

| Variant | When |
|---------|------|
| `NodeNotFound` | `node` not found on `PATH` |
| `NpmNotFound` | `npm` not found (only required when the script lists inline dependencies) |
| `ScriptNotFound(String)` | Script path does not exist |
| `ExecutionFailed(String)` | Spawn/wait, `npm install`, or other execution failure |
| `IoError(String)` | I/O or callback errors |
| `ExecutionTimedOut(u64)` | Wall-clock timeout |
| `OutputTruncated(usize)` | stdout/stderr exceeded `max_output_bytes` |
| `InvalidScriptMetadata(String)` | `// /// script` TOML could not be parsed |

---

## `JsExecutionService`

Stateless service type; construct with `JsExecutionService::new()`.

| Method | Returns | Description |
|--------|---------|-------------|
| `execute_script(&Path)` | `Result<ExecutionResult, JsExecutionError>` | `node <path>` (or temp workspace + `npm install` when inline deps are present) |
| `execute_code(&str)` | Same | Writes code to a temp `.mjs` file, then `execute_script` |
| `execute_script_streaming(&Path, on_output)` | Same | Per-line stdout callback |
| `execute_code_streaming(&str, on_output)` | Same | Temp `.mjs`, then `execute_script_streaming` |

Scripts may use a **`// /// script`** comment block with `dependencies = ["pkg"]` (npm specs).

---

## `DockerError`

`src/service/docker/error.rs` — `thiserror` enum:

| Variant | Meaning |
|---------|---------|
| `ConnectionFailed(String)` | Could not connect to Docker |
| `BuildFailed(String)` | Build-related failure (reserved / future use) |
| `ImageNotFound(String)` | Image missing or inspect/list failed |
| `ContainerCreationFailed(String)` | `create_container` failed |
| `ContainerStartFailed(String)` | Start or restart failed |
| `ExecutionFailed(String)` | Exec, logs, wait, or remove failed |
| `IoError(String)` | Generic I/O string |
| `InvalidConfig(String)` | `DockerConfig::validate` or bad regex setup |

---

## `DockerConfig`

See [Docker guide](docker.md#dockerconfig). Implements `Default` and `Clone`; provides `validate() -> Result<(), DockerError>`.

---

## `DockerService`

Async type wrapping `bollard::Docker` + `DockerConfig`. All methods are `async` unless noted.

### Construction

| Method | Description |
|--------|-------------|
| `new(DockerConfig) -> Result<Self, DockerError>` | Validate config, connect (see [Docker guide](docker.md#connecting-to-the-daemon)) |
| `with_defaults() -> Result<Self, DockerError>` | `DockerConfig::default()` |

### Images and containers

| Method | Description |
|--------|-------------|
| `image_exists() -> Result<bool, DockerError>` | Inspect / list / heuristic probe |
| `create_container(cmd, env)` | Create container; returns **container ID** |
| `create_container_with_mounts(cmd, env, mounts)` | Bind mounts: `Vec<(host_path, container_path)>` |
| `start_container(&id)` | Start by ID |
| `is_container_running(&id) -> Result<bool, DockerError>` | Inspect state |
| `ensure_container_running(&id)` | Start if not running |
| `remove_container(&id, force)` | Remove container |

### Exec in existing container

| Method | Description |
|--------|-------------|
| `exec_in_container(id, cmd)` | `docker exec` style; returns `(stdout, stderr, exit_code)` |
| `exec_in_container_with_env(id, cmd, env)` | Same with optional env vector |

### Run new container to completion

| Method | Description |
|--------|-------------|
| `run_command(cmd, env)` | Create → start → wait → logs → exit code; removes container |
| `run_command_with_mounts(cmd, env, mounts)` | Same with mounts; **removes** container after |
| `run_command_with_mounts_and_cleanup(cmd, env, mounts, remove_after)` | If `remove_after` is `false`, container is left running |
| `run_command_with_mounts_keep_alive(...)` | Does **not** remove; returns `(stdout, stderr, exit_code, container_id)` |

### Python helpers

| Method | Description |
|--------|-------------|
| `execute_python_code(code)` | `exec-code --code` in a new container (default cleanup) |
| `execute_python_code_with_mounts(code, mounts)` | With bind mounts |
| `execute_python_code_with_mounts_and_cleanup(code, mounts, remove_after)` | Control removal |
| `execute_python_code_in_container(id, code)` | Exec `sandy exec-code --code` in a running container |
| `execute_python_code_in_container_with_env(id, code, env)` | Same with env |
| `execute_python_script(script_path)` | `exec-code --file` in a new container (mounting is caller’s responsibility) |
| `create_persistent_container(mounts)` | Entrypoint override + `sleep infinity`; returns container ID |

### JavaScript helpers

| Method | Description |
|--------|-------------|
| `execute_javascript_code(code)` | `exec-js --code` in a new container |
| `execute_javascript_code_with_mounts(code, mounts)` | With bind mounts |
| `execute_javascript_code_in_container(id, code)` | Exec `sandy exec-js --code` in a running container |

---

## Python package (`import sandy`)

The **`sandy`** wheel wraps native Docker and Firecracker functions and applies optional script preparation. See the [README](../README.md) Python section.

---

## `FirecrackerConfig` and `FirecrackerService`

These types require the `firecracker` feature and Linux KVM.

`FirecrackerConfig::default()` reads `SANDY_FIRECRACKER`, `SANDY_FC_KERNEL`,
and `SANDY_FC_ROOTFS`. It defaults to one vCPU, 1024 MiB, and no network.

| Method | Description |
|--------|-------------|
| `FirecrackerService::new(config)` | Validate and create the service |
| `is_available()` | Check Linux, KVM, Firecracker, kernel, and rootfs |
| `execute_python_code(code)` | Run Python through the guest agent |
| `execute_javascript_code(code)` | Run JavaScript through the guest agent |
| `execute_shell(command, workspace_hex, env)` | Run a shell command and return the updated workspace archive |
| `start_vm(name)` | Boot and persist a named Firecracker VM |
| `status_vms(name)` | Read status for one or all persisted VMs |
| `stop_vm(name, force)` | Stop and remove a persisted VM |

---

## CLI re-exports

From `sandy::cli` (also exported at crate root):

- `Cli`, `Commands`
- `run_shell`, `run_py03`, `run_exec_code`, `run_js`, `run_exec_js`, `run_docker_exec`, `run_docker_exec_js` (`docker-exec*` are **async**)

Use these if you embed sandy’s behavior in another binary.

---

## Related

- [Architecture](architecture.md)
- [Development](development.md)
