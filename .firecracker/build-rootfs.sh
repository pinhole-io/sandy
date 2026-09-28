#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_DIR="${FIRECRACKER_OUTPUT_DIR:-${ROOT_DIR}/.firecracker/out}"
ROOTFS_PATH="${FIRECRACKER_ROOTFS:-${OUTPUT_DIR}/rootfs.ext4}"
IMAGE="${SANDY_FIRECRACKER_IMAGE:-sandy:latest}"
SIZE_MB="${FIRECRACKER_ROOTFS_SIZE_MB:-2048}"
TARGET="${FIRECRACKER_TARGET:-x86_64-unknown-linux-musl}"
MOUNT_DIR="$(mktemp -d)"
EXPORT_TAR="$(mktemp --suffix=.tar)"
CONTAINER_ID=""

cleanup() {
    if [[ -n "${CONTAINER_ID}" ]]; then
        docker rm -f "${CONTAINER_ID}" >/dev/null 2>&1 || true
    fi
    if mountpoint -q "${MOUNT_DIR}"; then
        umount "${MOUNT_DIR}" || true
    fi
    rm -rf "${MOUNT_DIR}" "${EXPORT_TAR}"
}
trap cleanup EXIT

if [[ "${EUID}" -ne 0 ]]; then
    printf 'Run this script as root because it creates and mounts an ext4 image.\n' >&2
    exit 1
fi

command -v docker >/dev/null || { printf 'docker is required\n' >&2; exit 1; }
command -v mkfs.ext4 >/dev/null || { printf 'mkfs.ext4 is required\n' >&2; exit 1; }
command -v mountpoint >/dev/null || { printf 'mountpoint is required\n' >&2; exit 1; }
[[ -f "${ROOT_DIR}/target/${TARGET}/release/sandy" ]] || {
    printf 'Build the guest binary first with:\n  cargo build --release --no-default-features --features firecracker --target %s\n' "${TARGET}" >&2
    exit 1
}

docker image inspect "${IMAGE}" >/dev/null || {
    printf "Build the base image first: docker build -f .docker/Dockerfile -t %s .\n" "${IMAGE}" >&2
    exit 1
}

mkdir -p "${OUTPUT_DIR}"
CONTAINER_ID="$(docker create "${IMAGE}")"
docker export --output "${EXPORT_TAR}" "${CONTAINER_ID}"
truncate -s "${SIZE_MB}M" "${ROOTFS_PATH}"
mkfs.ext4 -F -q "${ROOTFS_PATH}"
mount -o loop "${ROOTFS_PATH}" "${MOUNT_DIR}"
tar -xf "${EXPORT_TAR}" -C "${MOUNT_DIR}"
install -D -m 0755 \
    "${ROOT_DIR}/target/${TARGET}/release/sandy" \
    "${MOUNT_DIR}/usr/local/bin/sandy"

install -D -m 0755 /dev/stdin "${MOUNT_DIR}/sbin/init" <<'INIT'
#!/bin/sh
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mkdir -p /workspace
export PATH=/usr/local/bin:/usr/bin:/bin
export HOME=/workspace
export UV_PYTHON=3.13
export UV_PYTHON_INSTALL_DIR=/opt/uv-python
export SANDY_IN_FIRECRACKER=1
exec /usr/local/bin/sandy fc-agent
INIT

mkdir -p "${MOUNT_DIR}/etc"
printf 'sandy-firecracker\n' > "${MOUNT_DIR}/etc/hostname"
if [[ -n "${SUDO_UID:-}" && -n "${SUDO_GID:-}" ]]; then
    chown "${SUDO_UID}:${SUDO_GID}" "${ROOTFS_PATH}"
fi
sync
printf 'Firecracker rootfs: %s\n' "${ROOTFS_PATH}"
