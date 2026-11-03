"""Sandy: Docker-isolated Python and JavaScript execution (native ``_native`` + script prep)."""

from __future__ import annotations

from collections.abc import Sequence

try:
    from sandy._native import execute_python_docker as _execute_python_docker_native
except ImportError:  # native built without docker feature
    _execute_python_docker_native = None  # type: ignore[misc, assignment]

try:
    from sandy._native import execute_javascript_docker as _execute_javascript_docker_native
except ImportError:  # native built without docker feature
    _execute_javascript_docker_native = None  # type: ignore[misc, assignment]

try:
    from sandy._native import execute_python_firecracker as _execute_python_firecracker_native
    from sandy._native import execute_javascript_firecracker as _execute_javascript_firecracker_native
except ImportError:  # native built without firecracker feature
    _execute_python_firecracker_native = None  # type: ignore[misc, assignment]
    _execute_javascript_firecracker_native = None  # type: ignore[misc, assignment]

from sandy.script_prep import prepare_js_script, prepare_script

__all__ = [
    "execute_javascript_docker",
    "execute_python_docker",
    "execute_javascript_firecracker",
    "execute_python_firecracker",
    "prepare_js_script",
    "prepare_script",
]


def execute_python_docker(
    code: str,
    *,
    allow_network: bool = True,
    no_auto_build: bool = True,
    python_version: str = "3.13",
    image_name: str = "sandy",
    image_tag: str = "latest",
    global_dependencies: Sequence[str] | None = None,
    bootstrap_python: str = "",
) -> tuple[str, str, int]:
    """Run Python in Sandy's Docker image (same path as ``sandy docker-exec``).

    Optionally merges ``global_dependencies`` into PEP 723 metadata and prepends
    ``bootstrap_python`` after the script block; see :func:`prepare_script`.
    """
    if _execute_python_docker_native is None:
        raise RuntimeError(
            "execute_python_docker requires sandy._native built with the docker feature"
        )
    prepared = prepare_script(
        code,
        global_dependencies=tuple(global_dependencies or ()),
        bootstrap_python=bootstrap_python or "",
    )
    return _execute_python_docker_native(
        prepared,
        allow_network=allow_network,
        no_auto_build=no_auto_build,
        python_version=python_version,
        image_name=image_name,
        image_tag=image_tag,
    )


def execute_javascript_docker(
    code: str,
    *,
    allow_network: bool = True,
    no_auto_build: bool = True,
    python_version: str = "3.13",
    image_name: str = "sandy",
    image_tag: str = "latest",
    global_dependencies: Sequence[str] | None = None,
    bootstrap_javascript: str = "",
) -> tuple[str, str, int]:
    """Run JavaScript in Sandy's Docker image (same path as ``sandy docker-exec-js``).

    Optionally merges ``global_dependencies`` into ``// /// script`` metadata and prepends
    ``bootstrap_javascript`` after the script block; see :func:`prepare_js_script`.
    """
    prepared = prepare_js_script(
        code,
        global_dependencies=tuple(global_dependencies or ()),
        bootstrap_javascript=bootstrap_javascript or "",
    )
    if _execute_javascript_docker_native is None:
        raise RuntimeError(
            "execute_javascript_docker requires sandy._native built with the docker feature"
        )
    return _execute_javascript_docker_native(
        prepared,
        allow_network=allow_network,
        no_auto_build=no_auto_build,
        python_version=python_version,
        image_name=image_name,
        image_tag=image_tag,
    )


def execute_python_firecracker(
    code: str,
    *,
    kernel_path: str | None = None,
    rootfs_path: str | None = None,
    allow_network: bool = False,
    global_dependencies: Sequence[str] | None = None,
    bootstrap_python: str = "",
) -> tuple[str, str, int]:
    """Run Python in a Firecracker microVM."""
    if _execute_python_firecracker_native is None:
        raise RuntimeError(
            "execute_python_firecracker requires sandy._native built with the firecracker feature"
        )
    prepared = prepare_script(
        code,
        global_dependencies=tuple(global_dependencies or ()),
        bootstrap_python=bootstrap_python or "",
    )
    return _execute_python_firecracker_native(
        prepared,
        kernel_path=kernel_path,
        rootfs_path=rootfs_path,
        allow_network=allow_network,
    )


def execute_javascript_firecracker(
    code: str,
    *,
    kernel_path: str | None = None,
    rootfs_path: str | None = None,
    allow_network: bool = False,
    global_dependencies: Sequence[str] | None = None,
    bootstrap_javascript: str = "",
) -> tuple[str, str, int]:
    """Run JavaScript in a Firecracker microVM."""
    if _execute_javascript_firecracker_native is None:
        raise RuntimeError(
            "execute_javascript_firecracker requires sandy._native built with the firecracker feature"
        )
    prepared = prepare_js_script(
        code,
        global_dependencies=tuple(global_dependencies or ()),
        bootstrap_javascript=bootstrap_javascript or "",
    )
    return _execute_javascript_firecracker_native(
        prepared,
        kernel_path=kernel_path,
        rootfs_path=rootfs_path,
        allow_network=allow_network,
    )
