#!/bin/bash
# Build the sandy Docker image.
# Run this script from any directory. The build context is the repository root.
#
# Usage:
#   .docker/build.sh
#   PYTHON_VERSION=3.12 .docker/build.sh
#   PYTHON_VERSION=3.12 IMAGE_TAG=v1.0 .docker/build.sh
#
# Environment variables:
#   PYTHON_VERSION  - Python version to pre-install (default: 3.13)
#   IMAGE_NAME      - Docker image name (default: sandy)
#   IMAGE_TAG       - Docker image tag (default: latest)

set -e

# Default values (matching DockerConfig defaults)
IMAGE_NAME="${IMAGE_NAME:-sandy}"
IMAGE_TAG="${IMAGE_TAG:-latest}"
PYTHON_VERSION="${PYTHON_VERSION:-3.13}"

# Full image name
FULL_IMAGE_NAME="${IMAGE_NAME}:${IMAGE_TAG}"

# Script is in sandy/.docker; build context is the sandy/ directory (one level up)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SANDY_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DOCKERFILE_PATH="${SCRIPT_DIR}/Dockerfile"

# Change to sandy root for build context
cd "${SANDY_ROOT}"

echo "Building Docker image: ${FULL_IMAGE_NAME}"
echo "Dockerfile: ${DOCKERFILE_PATH}"
echo "Build context: ${SANDY_ROOT}"
echo "Python version: ${PYTHON_VERSION}"
echo ""

# Build the image
docker build \
    --build-arg PYTHON_VERSION="${PYTHON_VERSION}" \
    -f "${DOCKERFILE_PATH}" \
    -t "${FULL_IMAGE_NAME}" \
    .

echo ""
echo "Docker image built successfully: ${FULL_IMAGE_NAME}"

