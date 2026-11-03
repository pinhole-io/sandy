"""Merge global inline-script dependencies and inject bootstrap source before execution.

Trusted-caller input only: ``global_dependencies`` and bootstrap snippets run in the
same sandbox as the user script.
"""

from __future__ import annotations

import tomllib
from collections.abc import Sequence
from typing import Any


def prepare_script(
    source: str,
    *,
    global_dependencies: Sequence[str] = (),
    bootstrap_python: str = "",
) -> str:
    """Return ``source`` with optional merged deps and bootstrap.

    Order of effects:

    1. If ``global_dependencies`` is non-empty, merge into the PEP 723 ``# /// script``
       block (prepend a minimal block if absent).
    2. If ``bootstrap_python`` is non-empty, insert it immediately after the closing
       ``# ///`` of that block. If there is no PEP 723 block, prepend bootstrap at the
       start of the file.

    Callers that inject their own preamble (e.g. stdin/env setup) should run this
    first, then append their preamble after the returned string.
    """
    return _prepare_script(
        source,
        global_dependencies=global_dependencies,
        bootstrap=bootstrap_python,
        comment_prefix="#",
    )


def prepare_js_script(
    source: str,
    *,
    global_dependencies: Sequence[str] = (),
    bootstrap_javascript: str = "",
) -> str:
    """Same as :func:`prepare_script` using ``// /// script`` (JavaScript comments)."""
    return _prepare_script(
        source,
        global_dependencies=global_dependencies,
        bootstrap=bootstrap_javascript,
        comment_prefix="//",
    )


def _prepare_script(
    source: str,
    *,
    global_dependencies: Sequence[str],
    bootstrap: str,
    comment_prefix: str,
) -> str:
    out = source
    if global_dependencies:
        out = merge_global_dependencies(
            out, global_dependencies, comment_prefix=comment_prefix
        )
    if bootstrap:
        out = inject_bootstrap_after_script_block(
            out, bootstrap, comment_prefix=comment_prefix
        )
    return out


def merge_global_dependencies(
    source: str,
    global_dependencies: Sequence[str],
    *,
    comment_prefix: str = "#",
) -> str:
    """Merge package specs into the script metadata block, or prepend a new block."""
    if not global_dependencies:
        return source
    extra = list(global_dependencies)
    lines = source.splitlines(keepends=True)
    bounds = _find_script_block(lines, comment_prefix)
    if bounds is None:
        return _emit_script_block({"dependencies": _merge_dep_lists([], extra)}, comment_prefix) + source

    start, end = bounds
    inner = lines[start + 1 : end]
    toml_src = _inner_lines_to_toml(inner, comment_prefix).strip()
    if not toml_src:
        data: dict[str, Any] = {}
    else:
        try:
            data = tomllib.loads(toml_src)
        except tomllib.TOMLDecodeError as e:
            raise ValueError(
                "Invalid TOML inside script metadata block; cannot merge global_dependencies"
            ) from e

    script_deps = data.get("dependencies")
    if script_deps is None:
        normalized: list[str] = []
    elif isinstance(script_deps, list):
        normalized = [str(x) for x in script_deps]
    else:
        raise TypeError("script metadata 'dependencies' must be a list of strings")
    data["dependencies"] = _merge_dep_lists(normalized, extra)
    new_block = _emit_script_block(data, comment_prefix)
    return "".join(lines[:start]) + new_block + "".join(lines[end + 1 :])


def inject_bootstrap_after_pep723(source: str, bootstrap_python: str) -> str:
    """Insert ``bootstrap_python`` after the PEP 723 block, or at file start if no block."""
    return inject_bootstrap_after_script_block(
        source, bootstrap_python, comment_prefix="#"
    )


def inject_bootstrap_after_script_block(
    source: str,
    bootstrap: str,
    *,
    comment_prefix: str,
) -> str:
    """Insert ``bootstrap`` after the script metadata block, or at file start if no block."""
    if not bootstrap:
        return source
    lines = source.splitlines(keepends=True)
    if not lines:
        return _ensure_trailing_newline(bootstrap)

    start = _script_block_open(comment_prefix)
    close = _script_block_close(comment_prefix)
    first = lines[0].strip()
    if first == start or first.replace(" ", "") == start.replace(" ", ""):
        for i, line in enumerate(lines[1:], start=1):
            if line.strip() == close or line.strip() == close.replace(" ", ""):
                insert_at = i + 1
                boot = _ensure_trailing_newline(bootstrap)
                return "".join(lines[:insert_at]) + boot + "".join(lines[insert_at:])

    return _ensure_trailing_newline(bootstrap) + source


def _ensure_trailing_newline(s: str) -> str:
    return s if s.endswith("\n") else s + "\n"


def _merge_dep_lists(script_deps: list[str], global_deps: Sequence[str]) -> list[str]:
    seen: set[str] = set()
    out: list[str] = []
    for d in script_deps:
        if d not in seen:
            seen.add(d)
            out.append(d)
    for d in global_deps:
        if d not in seen:
            seen.add(d)
            out.append(d)
    return out


def _script_block_open(comment_prefix: str) -> str:
    return f"{comment_prefix} /// script"


def _script_block_close(comment_prefix: str) -> str:
    return f"{comment_prefix} ///"


def _find_script_block(lines: list[str], comment_prefix: str) -> tuple[int, int] | None:
    if not lines:
        return None
    start = _script_block_open(comment_prefix)
    close = _script_block_close(comment_prefix)
    first = lines[0].strip()
    if first != start and first.replace(" ", "") != start.replace(" ", ""):
        return None
    for i, line in enumerate(lines[1:], start=1):
        stripped = line.strip()
        if stripped == close or stripped == close.replace(" ", ""):
            return (0, i)
    return None


def _inner_lines_to_toml(inner: list[str], comment_prefix: str) -> str:
    parts: list[str] = []
    prefix = comment_prefix
    spaced = prefix + " "
    for line in inner:
        raw = line.rstrip("\n\r").lstrip()
        if raw.startswith(spaced):
            parts.append(raw[len(spaced) :])
        elif raw.startswith(prefix):
            rest = raw[len(prefix) :]
            if rest.startswith(" "):
                rest = rest[1:]
            parts.append(rest)
        else:
            parts.append(raw)
    return "\n".join(parts)


def _toml_string(s: str) -> str:
    escaped = s.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _toml_inline_value(v: Any) -> str:
    if isinstance(v, str):
        return _toml_string(v)
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v)
    if isinstance(v, float):
        return repr(v)
    if isinstance(v, list):
        return "[" + ", ".join(_toml_inline_value(x) for x in v) + "]"
    raise TypeError(f"Unsupported TOML value type for script metadata re-emission: {type(v)!r}")


def _emit_script_block(data: dict[str, Any], comment_prefix: str) -> str:
    lines_out = [f"{comment_prefix} /// script\n"]
    for key, value in data.items():
        lines_out.append(f"{comment_prefix} {key} = {_toml_inline_value(value)}\n")
    lines_out.append(f"{comment_prefix} ///\n")
    return "".join(lines_out)
