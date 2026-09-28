#!/usr/bin/env bash
set -euo pipefail

VERSION="${FIRECRACKER_VERSION:-1.11.0}"
ARCH="${FIRECRACKER_ARCH:-x86_64}"
INSTALL_DIR="${FIRECRACKER_INSTALL_DIR:-${HOME}/.local/bin}"
BASE_URL="https://github.com/firecracker-microvm/firecracker/releases/download/v${VERSION}"
ARCHIVE_NAME="firecracker-v${VERSION}-${ARCH}.tgz"
TEMP_DIR="$(mktemp -d)"
ARCHIVE_PATH="${TEMP_DIR}/${ARCHIVE_NAME}"

cleanup() {
    rm -rf "${TEMP_DIR}"
}
trap cleanup EXIT

if [[ "${OSTYPE}" != linux* ]]; then
    printf 'Firecracker installation requires Linux.\n' >&2
    exit 1
fi

command -v curl >/dev/null || {
    printf 'curl is required\n' >&2
    exit 1
}
command -v tar >/dev/null || {
    printf 'tar is required\n' >&2
    exit 1
}

printf 'Downloading Firecracker %s for %s...\n' "${VERSION}" "${ARCH}"
curl --fail --location --retry 3 \
    --output "${ARCHIVE_PATH}" \
    "${BASE_URL}/${ARCHIVE_NAME}"

BINARY_MEMBER="$(
    tar -tzf "${ARCHIVE_PATH}" |
        awk -v arch="${ARCH}" '$0 ~ /firecracker-v/ && $0 ~ ("-" arch "$") { print; exit }'
)"
if [[ -z "${BINARY_MEMBER}" ]]; then
    printf 'Could not find the Firecracker binary in %s\n' "${ARCHIVE_NAME}" >&2
    exit 1
fi

mkdir -p "${INSTALL_DIR}"
TEMP_BINARY="${TEMP_DIR}/firecracker"
tar -xOzf "${ARCHIVE_PATH}" "${BINARY_MEMBER}" > "${TEMP_BINARY}"
install -m 0755 "${TEMP_BINARY}" "${INSTALL_DIR}/firecracker"
printf 'Installed Firecracker at %s/firecracker\n' "${INSTALL_DIR}"
