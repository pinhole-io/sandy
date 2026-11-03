# Docker guide

This document covers Docker connectivity, `DockerConfig`, building the runtime image, execution patterns, XML output from the CLI, and internal timeouts.

## `DockerConfig`

Defined in `src/service/docker/config.rs`. Fields:

| Field | Default | Purpose |
|-------|---------|---------|
| `image_name` | `sandy` | Repository/name part of `image:tag` |
| `image_tag` | `latest` | Tag part |
| `dockerfile_path` | `src/sandy/.docker/Dockerfile` | Path stored by `DockerConfig::default()`. The CLI picks a file that exists. See [Building the image](#building-the-image). |
| `build_context` | `.` | Default build context directory when building from repo root |

### Validation (`validate()`)

- **Image name** must match: `^[a-zA-Z0-9][a-zA-Z0-9._/-]*$`
- **Tag** must match: `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`

On failure, `DockerError::InvalidConfig` is returned.

## Connecting to the daemon

`DockerService::new` resolves the socket in this order:

1. **`DOCKER_HOST`** — if set, connect with `Docker::connect_with_socket` (timeout **120s**).
2. **`$HOME/.config/colima/default/docker.sock`** — if the file exists (new Colima layout).
3. **`$HOME/.colima/default/docker.sock`** — if the file exists (legacy Colima layout).
4. **`Docker::connect_with_local_defaults()`** — typical Linux `/var/run/docker.sock` or Docker Desktop defaults.

API version: bollard’s **`API_DEFAULT_VERSION`** for the client.

## Building the image

The Dockerfile is [`.docker/Dockerfile`](../.docker/Dockerfile).

- The builder image is Rust nightly. The binary target is musl.
- The runtime image is Alpine. It includes `curl`, `bash`, `uv`, Python, Node.js, and npm.
- The default Python version is 3.13. Pass a version that `uv` ships as a musl build.

Tag the image `sandy:latest` unless you pass `--image` and `--tag` to `docker-exec`.

### Commands

Run these commands from this repository. The build context is the repository root.

```bash
docker build -f .docker/Dockerfile -t sandy:latest .
```

Optional build argument: `--build-arg PYTHON_VERSION=3.12`.

[`.docker/build.sh`](../.docker/build.sh) runs that build. You can start it from any directory. The script uses this repository as the build context.

```bash
.docker/build.sh
```

### `build.sh` environment variables

| Variable | Default | Description |
| --- | --- | --- |
| `PYTHON_VERSION` | `3.13` | Python version passed to `uv python install`. It must have a musl build. |
| `IMAGE_NAME` | `sandy` | Image name |
| `IMAGE_TAG` | `latest` | Image tag |

```bash
PYTHON_VERSION=3.12 IMAGE_TAG=dev .docker/build.sh
```

`DockerConfig::default()` still stores `src/sandy/.docker/Dockerfile`. The CLI does not use that string when a candidate file exists. Candidate order:

1. `.docker/Dockerfile`
2. `sandy/.docker/Dockerfile`
3. `src/sandy/.docker/Dockerfile`

Pass `--dockerfile` when you build from another directory. Set `DockerConfig.dockerfile_path` when you call `DockerService` from Rust.

## Network isolation vs outbound access

`SandboxConfig` **`network_disabled`** defaults to **`true`**: Docker [`HostConfig`](https://docs.docker.com/engine/api/v1.44/#tag/Container/operation/ContainerCreate) uses **`network_mode=none`**, so the container has **no outbound** network (no PyPI, no GitHub downloads for `uv` Python builds).

- **CLI**: pass **`docker-exec --allow-network`** (or **`docker-exec-js --allow-network`**) to set `network_disabled=false` (default bridge network, outbound allowed). Use this when inline `uv` / npm dependencies need the internet.
- **Library**: `DockerConfig { sandbox: SandboxConfig { network_disabled: false, .. }, .. }`.

For maximum isolation, omit `--allow-network` and keep the default.

### Writable `/workspace` (tmpfs ownership)

At runtime, **`/workspace`** is a **tmpfs** mount so the root filesystem can stay read-only. The mount uses **`uid=1000,gid=1000`** so the **`sandbox`** user (see Dockerfile) can create **`/workspace/.cache`** and **`uv`** cache dirs. If you change the image user’s numeric uid/gid, update the tmpfs options in `DockerService::build_host_config` to match.

## How Python runs in Docker

- **One-shot**: `DockerService::execute_python_code` runs a container with command `exec-code --code <code>`, which maps to the image entrypoint + args. The container is removed after completion when using the default cleanup path.
- **Persistent**: `create_persistent_container` overrides the image entrypoint to `/bin/sh -c 'sleep infinity'`, starts the container, and returns its ID. Further runs use `execute_python_code_in_container`, which runs `sandy exec-code --code ...` via `docker exec` (see source for exact argv).

`execute_python_script` (library) uses `exec-code --file <path>` inside a new container; mounting the script is left to the caller’s volume strategy.

**Third-party imports** (`httpx`, `requests`, …): use **`--allow-network`** when PyPI is needed, and put [uv PEP 723 script metadata](https://docs.astral.sh/uv/guides/scripts/#declaring-script-dependencies) at the **start** of the code (not `# uv: …`). Minimal example:

```text
# /// script
# dependencies = ["httpx"]
# ///
import httpx
print(httpx.get("https://httpbin.org/get", timeout=10.0).status_code)
```

## Timeouts and polling (implementation constants)

These are defined on `DockerService` in `src/service/docker/service.rs`:

| Constant | Value | Role |
|----------|-------|------|
| `CONTAINER_TIMEOUT_SECS` | 300 | Max wait for container exit (5 minutes) |
| `MAX_POLL_ATTEMPTS` | 60 | Poll attempts when wait stream fails |
| `POLL_INTERVAL_MS` | 500 | Sleep between polls |
| `DOCKER_CONNECT_TIMEOUT_SECS` | 120 | Socket connect timeout |

If the container does not reach a terminal state within the poll loop, `DockerError::ExecutionFailed` with *"Container did not finish within timeout"* may be returned.

## XML output from `docker-exec`

The **`docker-exec`** subcommand prints a single XML document to **stdout**:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<execution>
  <output>
    <stdout>...</stdout>
    <stderr>...</stderr>
  </output>
  <metrics>
    <start_time>YYYY-MM-DD HH:MM:SS UTC</start_time>
    <duration_seconds>...</duration_seconds>
    <duration_milliseconds>...</duration_milliseconds>
    <exit_code>...</exit_code>
  </metrics>
</execution>
```

- **Escaping**: `&`, `<`, `>`, `"`, `'` in stdout/stderr are XML-escaped.
- **Times**: Start time is UTC from the wall clock at invocation; duration is measured around the async `execute_python_code` call.

## Checking images

`DockerService::image_exists` uses `inspect_image`, then listing tags, and as a last resort a short-lived test container creation to detect edge cases.

## Related

- [CLI reference – `docker-exec`](cli-reference.md#docker-exec) / [`docker-exec-js`](cli-reference.md#docker-exec-js)
- [Firecracker guide](firecracker.md) for Linux KVM microVM isolation
- [Library API – `DockerService`](api.md#dockerservice)
