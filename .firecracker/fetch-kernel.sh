#!/usr/bin/env bash
set -euo pipefail

# Firecracker publishes kernel images in dated CI artifacts. Resolve the
# current artifact because old fixed kernel URLs are retired.
ARCH="${FIRECRACKER_ARCH:-$(uname -m)}"
OUTPUT_DIR="${FIRECRACKER_OUTPUT_DIR:-.firecracker/out}"
OUTPUT_PATH="${OUTPUT_DIR}/vmlinux"
S3="https://s3.amazonaws.com/spec.ccfc.min"

if [[ -n "${FIRECRACKER_KERNEL_URL:-}" ]]; then
    URL="${FIRECRACKER_KERNEL_URL}"
else
    if [[ -n "${FIRECRACKER_KERNEL_VERSION:-}" ]]; then
        CI_ARTIFACTS_PREFIX="firecracker-ci/${FIRECRACKER_KERNEL_VERSION}/${ARCH}/"
    else
        CI_ARTIFACTS_PREFIX="$(
            curl --fail --location --silent --show-error \
                "${S3}?list-type=2&prefix=firecracker-ci/&delimiter=/" |
                awk -F'[<>]' '{for (i = 1; i <= NF; i++) if ($i == "Prefix") print $(i + 1)}' |
                awk -F/ '$2 ~ /^[0-9]{8}-/ {print}' |
                sort |
                tail -n 1
        )${ARCH}/"
    fi

    if [[ -n "${FIRECRACKER_KERNEL_ASSET:-}" ]]; then
        KERNEL_KEY="${CI_ARTIFACTS_PREFIX}${FIRECRACKER_KERNEL_ASSET}"
    else
        KERNEL_PREFIX="${CI_ARTIFACTS_PREFIX}vmlinux-"
        KERNEL_KEY="$(
            curl --fail --location --silent --show-error \
                "${S3}?list-type=2&prefix=${KERNEL_PREFIX}" |
                awk -F'[<>]' '{for (i = 1; i <= NF; i++) if ($i == "Key" && $(i + 1) ~ /\/vmlinux-[0-9]+\.[0-9]+\.[0-9]+$/) print $(i + 1)}' |
                sort -V |
                tail -n 1
        )"
    fi

    if [[ -z "${KERNEL_KEY}" ]]; then
        printf 'No Firecracker kernel found for %s\n' "${CI_ARTIFACTS_PREFIX}" >&2
        exit 1
    fi
    URL="${S3}/${KERNEL_KEY}"
fi

mkdir -p "${OUTPUT_DIR}"
curl --fail --location --retry 3 --output "${OUTPUT_PATH}.tmp" "${URL}"
mv "${OUTPUT_PATH}.tmp" "${OUTPUT_PATH}"
chmod 0644 "${OUTPUT_PATH}"
printf 'Firecracker kernel: %s\n' "${OUTPUT_PATH}"
