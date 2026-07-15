#!/usr/bin/env python3
import hashlib
import lzma
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import apt_pkg

apt_pkg.init_system()


def fail(message):
    raise SystemExit("ubuntu Debian evidence: " + message)


def fields(raw):
    result = {}
    current = None
    for line in raw.splitlines():
        if line.startswith((" ", "\t")) and current:
            result[current] += "\n" + line
        elif ": " in line:
            current, value = line.split(": ", 1)
            result[current] = value
    return result


def canonical(path, lines, mode):
    rendered = "\n".join(sorted(set(lines))) + "\n"
    if mode == "--write":
        path.write_text(rendered)
    elif not path.is_file() or path.read_text() != rendered:
        fail(path.name + " differs from independent APT recomputation")


def selected_packages(root):
    upstream = []
    for index in sorted((root / "signed-indexes").rglob("Packages.xz")):
        upstream.extend(
            fields(raw)
            for raw in lzma.decompress(index.read_bytes()).decode().strip().split("\n\n")
        )
    result = {}
    for line in (root / "package-manifest.tsv").read_text().splitlines():
        name, version, arch, filename, sha, size = line.split("\t")
        matches = [
            item
            for item in upstream
            if (
                item.get("Package"),
                item.get("Version"),
                item.get("Architecture"),
                item.get("Filename"),
                item.get("SHA256"),
                item.get("Size"),
            )
            == (name, version, arch, filename, sha, size)
        ]
        if len(matches) != 1:
            fail("selection is not uniquely present in signed Packages")
        result[(name, version, arch)] = matches[0]
    return result


def recompute(root, selected):
    by_name = {}
    providers = {}
    for key, item in selected.items():
        by_name.setdefault(key[0], []).append(key)
        for group in apt_pkg.parse_depends(item.get("Provides", ""), False, "arm64"):
            for provided, version, _operator in group:
                providers.setdefault(provided.split(":", 1)[0], []).append((key, version))
    requirements = []
    edges = []
    for source, item in sorted(selected.items()):
        for relation in ("Depends", "Pre-Depends"):
            raw = item.get(relation, "")
            if not raw:
                continue
            parsed = apt_pkg.parse_depends(raw, False, "arm64")
            expressions = [part.strip() for part in raw.split(",")]
            if len(parsed) != len(expressions):
                fail("APT dependency normalization mismatch")
            for index, (expression, alternatives) in enumerate(zip(expressions, parsed)):
                requirement = (*source, relation, str(index), expression)
                requirements.append("\t".join(requirement))
                candidates = []
                for position, (target, required, operator) in enumerate(alternatives):
                    base = target.split(":", 1)[0]
                    for key in by_name.get(base, []):
                        candidate = selected[key]
                        if not operator or apt_pkg.check_dep(key[1], operator, required):
                            candidates.append((position, key))
                    for key, provided in providers.get(base, []):
                        candidate = selected[key]
                        if not operator or (provided and apt_pkg.check_dep(provided, operator, required)):
                            candidates.append((position, key))
                if not candidates:
                    fail("selected set does not satisfy " + expression)
                chosen = min(candidates, key=lambda value: (value[0], value[1]))[1]
                edges.append("\t".join((*requirement, *chosen)))
    return requirements, edges


def offline_check(root, selected):
    with tempfile.TemporaryDirectory(prefix="gascan-apt-check-") as temporary:
        state = Path(temporary)
        for path in (state / "lists/partial", state / "cache/archives/partial"):
            path.mkdir(parents=True)
        sources = state / "sources.list"
        sources.write_text(f"deb [trusted=yes] file:{root / 'repository'} gascan main\n")
        options = [
            "-o", f"Dir::Etc::sourcelist={sources}", "-o", "Dir::Etc::sourceparts=-",
            "-o", f"Dir::State::lists={state / 'lists'}", "-o", f"Dir::Cache={state / 'cache'}",
            "-o", "Dir::State::status=/dev/null", "-o", "APT::Architecture=arm64",
            "-o", "APT::Install-Recommends=false", "-o", "Acquire::Retries=0",
            "-o", "Acquire::http::Proxy=false", "-o", "Acquire::https::Proxy=false",
            "-o", "Dir::Bin::Methods::http=/bin/false", "-o", "Dir::Bin::Methods::https=/bin/false",
        ]
        update = subprocess.run(["apt-get", *options, "update"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if update.returncode:
            fail("isolated local repository update failed: " + update.stderr)
        exact = [f"{name}={version}" if arch == "all" else f"{name}:{arch}={version}" for name, version, arch in sorted(selected)]
        solve = subprocess.run(["apt-get", *options, "--simulate", "--no-download", "--no-install-recommends", "install", *exact], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if solve.returncode:
            fail("isolated exact offline APT resolution failed: " + solve.stderr)
        return ["selection-sha256\t" + hashlib.sha256("\n".join(exact).encode()).hexdigest(), "apt-simulation\tpassed"]


if len(sys.argv) != 3 or sys.argv[1] not in ("--write", "--verify"):
    fail("usage: verify-ubuntu-debian-evidence.py --write|--verify EVIDENCE")
mode, root = sys.argv[1], Path(sys.argv[2])
selected = selected_packages(root)
requirements, edges = recompute(root, selected)
canonical(root / "dependency-requirements.tsv", requirements, mode)
canonical(root / "dependency-edges.tsv", edges, mode)
canonical(root / "offline-apt-check.tsv", offline_check(root, selected), mode)
