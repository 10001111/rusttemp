#!/usr/bin/env python3
"""Write THIRD_PARTY_LICENSES.md for a RustTemp release build.

Lists every crate that is compiled into the release binaries for one target,
with the licence texts shipped in each crate (identical texts are printed
once). For Windows targets it also includes the bundled PawnIO module.

Usage:
    python scripts/third_party_licenses.py --target x86_64-pc-windows-msvc --out THIRD_PARTY_LICENSES.md
    python scripts/third_party_licenses.py --target x86_64-unknown-linux-musl --no-default-features --out THIRD_PARTY_LICENSES.md
"""

import argparse
import json
import pathlib
import subprocess

LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "unlicense")
ROOT = pathlib.Path(__file__).resolve().parent.parent


def crate_license_files(manifest_path):
    folder = pathlib.Path(manifest_path).parent
    files = []
    for f in sorted(folder.iterdir()):
        if f.is_file() and f.name.lower().startswith(LICENSE_PREFIXES):
            files.append(f)
    return files


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    ap.add_argument("--no-default-features", action="store_true")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    cmd = ["cargo", "metadata", "--format-version", "1", "--locked",
           "--filter-platform", args.target]
    if args.no_default_features:
        cmd.append("--no-default-features")
    meta = json.loads(subprocess.check_output(cmd, cwd=ROOT))

    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]

    # Normal (non-dev, non-build) dependencies reachable from RustTemp.
    seen, stack = set(), [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    seen.discard(root)
    crates = sorted((packages[p] for p in seen), key=lambda p: (p["name"], p["version"]))

    texts = {}          # licence text -> list of "name version"
    no_file = []
    for p in crates:
        label = f'{p["name"]} {p["version"]}'
        files = crate_license_files(p["manifest_path"])
        if not files:
            no_file.append(p)
        for f in files:
            body = f.read_text(encoding="utf-8", errors="replace").strip()
            texts.setdefault(body, []).append(label)

    out = []
    out.append("# Third-party licences\n")
    out.append("RustTemp itself is licensed under the PolyForm Noncommercial License "
               "1.0.0 (see `LICENSE.md`). The RustTemp programs in this download also "
               "contain the third-party components listed below, which remain under "
               "their own licences.\n")
    out.append(f"Target: `{args.target}`\n")
    out.append("## Rust crates\n")
    out.append("| Crate | Version | Licence | Source |")
    out.append("|---|---|---|---|")
    for p in crates:
        src = p.get("repository") or f'https://crates.io/crates/{p["name"]}'
        out.append(f'| {p["name"]} | {p["version"]} | {p.get("license") or "see below"} | {src} |')
    out.append("")

    if "windows" in args.target:
        copying = (ROOT / "assets" / "pawnio" / "COPYING").read_text(encoding="utf-8").strip()
        out.append("## PawnIO IntelMSR module\n")
        out.append("The Windows programs embed `IntelMSR.bin`, the signed PawnIO module that "
                   "reads Intel processor registers, unmodified from "
                   "<https://github.com/namazso/PawnIO.Modules> release 0.2.11 "
                   "(source: `IntelMSR.p` in that repository). It is licensed under the "
                   "GNU Lesser General Public License 2.1 or later:\n")
        out.append("```text\n" + copying + "\n```\n")

    out.append("## Licence texts\n")
    for body, users in sorted(texts.items(), key=lambda kv: kv[1][0]):
        out.append("Used by: " + ", ".join(sorted(set(users))) + "\n")
        out.append("```text\n" + body + "\n```\n")
    if no_file:
        out.append("The following crates ship no licence file; their declared licence "
                   "(SPDX expression) applies:\n")
        for p in no_file:
            out.append(f'- {p["name"]} {p["version"]}: {p.get("license")}')
        out.append("")

    pathlib.Path(args.out).write_text("\n".join(out), encoding="utf-8")
    print(f"wrote {args.out}: {len(crates)} crates, {len(texts)} distinct licence texts")


if __name__ == "__main__":
    main()
