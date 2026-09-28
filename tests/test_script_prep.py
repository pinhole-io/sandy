"""Tests for PEP 723 merge and bootstrap injection."""

from sandy.script_prep import (
    inject_bootstrap_after_pep723,
    merge_global_dependencies,
    prepare_js_script,
    prepare_script,
)


def test_prepare_noop_when_empty_options() -> None:
    src = """# /// script
# dependencies = ["httpx"]
# ///
print(1)
"""
    assert prepare_script(src) == src


def test_global_deps_prepends_block_when_missing() -> None:
    src = "print('hi')\n"
    out = prepare_script(src, global_dependencies=["pandas"])
    assert out.startswith("# /// script\n")
    assert '# dependencies = ["pandas"]' in out
    assert out.endswith(src)


def test_global_deps_merges_existing_block_preserves_requires_python() -> None:
    src = """# /// script
# dependencies = ["httpx"]
# requires-python = ">=3.12"
# ///
body
"""
    out = merge_global_dependencies(src, ["pandas", "httpx"])
    assert "# dependencies = " in out
    assert "httpx" in out
    assert "pandas" in out
    assert 'requires-python = ">=3.12"' in out
    assert out.endswith("body\n")


def test_bootstrap_after_pep723_block() -> None:
    src = """# /// script
# dependencies = []
# ///
print(1)
"""
    out = prepare_script(src, bootstrap_python="def helper():\n    return 2\n")
    assert "# ///\n" in out
    idx_close = out.index("# ///\n") + len("# ///\n")
    assert out[idx_close:].startswith("def helper()")


def test_bootstrap_only_no_pep723_prepends() -> None:
    src = "x = 1\n"
    out = prepare_script(src, bootstrap_python="import math\n")
    assert out.startswith("import math\n")
    assert "x = 1" in out


def test_execute_python_docker_accepts_prepare_kwargs() -> None:
    from unittest.mock import patch

    from sandy import execute_python_docker

    src = """# /// script
# dependencies = []
# ///
pass
"""
    with patch("sandy._execute_python_docker_native") as mock_native:
        mock_native.return_value = ("{}", "", 0)
        execute_python_docker(
            src,
            global_dependencies=["requests"],
            bootstrap_python="FOO = 1\n",
        )
        mock_native.assert_called_once()
        (code,), _kwargs = mock_native.call_args
        assert "requests" in code
        assert "FOO = 1" in code


def test_prepare_js_noop_when_empty_options() -> None:
    src = """// /// script
// dependencies = ["ms"]
// ///
console.log(1)
"""
    assert prepare_js_script(src) == src


def test_js_global_deps_prepends_block_when_missing() -> None:
    src = "console.log('hi')\n"
    out = prepare_js_script(src, global_dependencies=["ms"])
    assert out.startswith("// /// script\n")
    assert '// dependencies = ["ms"]' in out
    assert out.endswith(src)


def test_js_bootstrap_after_script_block() -> None:
    src = """// /// script
// dependencies = []
// ///
console.log(1)
"""
    out = prepare_js_script(src, bootstrap_javascript="const helper = 2;\n")
    assert "// ///\n" in out
    idx_close = out.index("// ///\n") + len("// ///\n")
    assert out[idx_close:].startswith("const helper = 2;")


def test_execute_javascript_docker_accepts_prepare_kwargs() -> None:
    from unittest.mock import patch

    from sandy import execute_javascript_docker

    src = """// /// script
// dependencies = []
// ///
console.log(1)
"""
    with patch("sandy._execute_javascript_docker_native") as mock_native:
        mock_native.return_value = ("{}", "", 0)
        execute_javascript_docker(
            src,
            global_dependencies=["ms"],
            bootstrap_javascript="const FOO = 1;\n",
        )
        mock_native.assert_called_once()
        (code,), _kwargs = mock_native.call_args
        assert "ms" in code
        assert "const FOO = 1" in code

