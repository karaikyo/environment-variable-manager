#!/usr/bin/env python3
"""Generate release notes from a matching version entry in CHANGELOG.md."""
import argparse
import pathlib
import re
import subprocess


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--changelog", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", args.tag):
        parser.error("expected a vMAJOR.MINOR.PATCH release tag")
    version = args.tag[1:]
    changelog = args.changelog.read_text(encoding="utf-8")
    headings = list(re.finditer(r"(?m)^## \[?([^\]\n]+)\]?(?: - [^\n]+)?\s*$", changelog))
    matching = [i for i, item in enumerate(headings) if item.group(1) == version]
    if len(matching) != 1:
        parser.error(f"CHANGELOG.md must contain exactly one ## [{version}] version section")
    index = matching[0]
    start = headings[index].end()
    end = headings[index + 1].start() if index + 1 < len(headings) else changelog.find("\n[Unreleased]:", start)
    if end < 0:
        end = len(changelog)
    details = changelog[start:end].strip()
    if not details:
        parser.error(f"empty changelog section for {version}")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    notes = (
        f"## Environment Manager {args.tag} (Preview)\n\n"
        f"{details}\n\n"
        "### Download and update\n\n"
        "Portable Windows 11 x64 preview. Extract into a writable folder and launch "
        "`environment-variable-manager.exe`. Exit the running application before "
        "updating; preserve `data/` when replacing application files.\n\n"
        "### Verification and source\n\n"
        "Windows CI functional tests and package/privacy validation passed. "
        "Native interactive UI, UAC, real sleep/hibernate and remaining controlled "
        "acceptance tests are not covered by these CI runs.\n\n"
        f"Source commit: `{revision}` (tag `{args.tag}`). "
        "SHA-256 checksum attached alongside the portable ZIP.\n"
    )
    args.output.write_text(notes, encoding="utf-8")


if __name__ == "__main__":
    main()
