# Firecracker

Firecracker runs Sandy code in a microVM with a separate Linux kernel.

## Requirements

- Linux with `/dev/kvm`.
- The `firecracker` binary on `PATH`.
- A Firecracker-compatible `vmlinux` kernel.
- An ext4 rootfs that contains the Sandy guest agent, `uv`, Python, and Node.js.
- `tar`, `mkfs.ext4`, and Docker for the rootfs helper.

Firecracker is enabled by default on Linux. Use `--no-default-features`
to build Sandy without it, or pass `--features firecracker` explicitly.

Install the pinned host binary and fetch the default kernel with:

```bash
make firecracker-install
```

Override the release or destination with `FIRECRACKER_VERSION`,
`FIRECRACKER_ARCH`, or `FIRECRACKER_INSTALL_DIR`.

## Build the guest artifacts

Run these commands from the Sandy crate root:

```bash
make firecracker-kernel
cargo build --release --no-default-features --features firecracker \
  --target x86_64-unknown-linux-musl
make firecracker-rootfs
```

The scripts write `.firecracker/out/vmlinux` and
`.firecracker/out/rootfs.ext4`.

The rootfs script exports `sandy:latest`. Build that image first:

```bash
docker build -f .docker/Dockerfile -t sandy:latest .
```

Set `SANDY_FIRECRACKER_IMAGE` to use another base image.

## Execute code

```bash
cargo run --features firecracker -- fc-exec \
  --kernel .firecracker/out/vmlinux \
  --rootfs .firecracker/out/rootfs.ext4 \
  --code 'print("hello from a microVM")'

cargo run --features firecracker -- fc-exec-js \
  --kernel .firecracker/out/vmlinux \
  --rootfs .firecracker/out/rootfs.ext4 \
  --code 'console.log("hello from a microVM")'
```

Both commands print the same XML structure as `docker-exec`.

You can set `SANDY_FC_KERNEL` and `SANDY_FC_ROOTFS` instead of passing paths.

## Manage persistent microVMs

Use `microvm start` to boot a VM without running code. Sandy stores the PID
and socket paths under `SANDY_FC_STATE_DIR` or
`~/.cache/sandy/firecracker/vms`.

```bash
sandy microvm start --name worker \
  --kernel .firecracker/out/vmlinux \
  --rootfs .firecracker/out/rootfs.ext4
sandy microvm status
sandy microvm status --name worker --json
sandy microvm stop --name worker
```

Use `--force` with `stop` to send `SIGKILL`. Start the VM again before using
it for another workload.

## Network policy

The default VM has no network interface. `--allow-network` requires a
pre-created TAP device in `SANDY_FC_TAP`. Sandy does not create NAT or
configure a host firewall.

## Harn

Build the Harn Sandy plugin with the `firecracker` feature enabled. Set:

```toml
[sandbox]
backend = "firecracker"
```

The shell backend archives the workspace into the VM and restores it after
execution. Writable roots outside the workspace are rejected.

## Security and limitations

- Firecracker provides a separate guest kernel and a KVM boundary.
- The v1 integration starts one VM per request.
- The v1 integration does not use `jailer`.
- The v1 integration does not provide automatic networking.
- The guest agent uses vsock port 52.
