"""Reproduces the diff body of a `patches/mocari-<version>-*.patch` file.

The prose header of a patch is hand-written and stays; only the unified diff
between the crates.io source and the committed copy at `vendor/mocari` is
generated. Append the output below a finished header:

    python scripts/mocari-patch.py src/render/wgpu/buffers.rs src/render/wgpu/draw.rs
"""

import difflib
import os
import pathlib
import sys

VERSION = "0.4.1"
VENDORED = pathlib.Path("vendor/mocari")


def registry_source(version: str) -> pathlib.Path:
    homes = [pathlib.Path(os.environ["CARGO_HOME"])] if os.environ.get("CARGO_HOME") else []
    homes.append(pathlib.Path.home() / ".cargo")
    for home in homes:
        found = sorted((home / "registry" / "src").glob(f"*/mocari-{version}"))
        if found:
            return found[0]
    raise SystemExit(
        f"mocari-{version} is not in the cargo registry - run 'cargo fetch' first"
    )


def main(names: list[str]) -> None:
    source = registry_source(VERSION)
    for name in names:
        old = (source / name).read_text(encoding="utf-8").splitlines(keepends=True)
        new = (VENDORED / name).read_text(encoding="utf-8").splitlines(keepends=True)
        print(f"diff --git a/{name} b/{name}")
        print(
            "".join(
                difflib.unified_diff(old, new, fromfile=f"a/{name}", tofile=f"b/{name}", n=3)
            ),
            end="",
        )


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    main(sys.argv[1:])
