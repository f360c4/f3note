#!/usr/bin/env python3
"""Turn Cargo.lock into the source list a Flatpak build needs.

A Flatpak build has no network. Every crate therefore has to be declared
up front, with the checksum Cargo already recorded, so flatpak-builder can
fetch them before the sandbox closes and cargo can be run with
--offline against a vendor directory.

The checksums come from Cargo.lock rather than being fetched, which means
this cannot silently pin something other than what builds here.
"""
import json
import sys
import tomllib
from pathlib import Path

CRATES_IO = "https://static.crates.io/crates/{name}/{name}-{version}.crate"


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    lock = tomllib.loads((root / "Cargo.lock").read_text())

    sources = []
    vendored = []
    for package in lock.get("package", []):
        checksum = package.get("checksum")
        if not checksum:
            # No checksum means it is not from crates.io — the workspace
            # member itself, or a git or path dependency. Nothing to fetch.
            continue
        name, version = package["name"], package["version"]
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": CRATES_IO.format(name=name, version=version),
                "sha256": checksum,
                "dest": f"cargo/vendor/{name}-{version}",
            }
        )
        # Cargo expects a checksum file beside each vendored crate, and the
        # "package" field has to carry the same hash the lock file records.
        # Writing null there instead makes cargo refuse the crate with
        # "checksum could not be calculated, but a checksum is listed in the
        # existing lock file" — it reads null as a source that cannot do
        # checksums replacing one that can. The files map stays empty because
        # flatpak-builder unpacked the archive, so there is nothing to compare
        # file by file.
        vendored.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}, indent=2),
                "dest": f"cargo/vendor/{name}-{version}",
                "dest-filename": ".cargo-checksum.json",
            }
        )

    config = """[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "cargo/vendor"
"""
    sources.append(
        {
            "type": "inline",
            "contents": config,
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )

    out = root / "packaging/flatpak/cargo-sources.json"
    out.write_text(json.dumps(sources + vendored, indent=2) + "\n")
    print(f"{len(sources) - 1} crates -> {out.relative_to(root)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
