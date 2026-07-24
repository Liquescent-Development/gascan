#!/usr/bin/env python3
import hashlib
import lzma
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

import apt_pkg

apt_pkg.init_system()

REVIEWED_ROOT_PROVIDERS = {
    "libatk-bridge2.0-0": "libatk-bridge2.0-0t64",
    "libatk1.0-0": "libatk1.0-0t64",
    "libcups2": "libcups2t64",
}


def fail(message):
    raise SystemExit("ubuntu Debian evidence: " + message)


def fields(raw):
    result = {}
    current = None
    for line in raw.splitlines():
        if line.startswith((" ", "\t")) and current:
            result[current] += "\n" + line
        elif ":" in line:
            current, value = line.split(":", 1)
            if value.startswith(" "):
                value = value[1:]
            elif value:
                fail("invalid signed Packages field")
            if current in result:
                fail("duplicate signed Packages field")
            result[current] = value
        else:
            fail("invalid signed Packages field")
    return result


def canonical(path, lines, mode):
    rendered = "\n".join(sorted(set(lines))) + "\n"
    if mode == "--write":
        path.write_text(rendered)
    elif not path.is_file() or path.read_text() != rendered:
        fail(path.name + " differs from independent APT recomputation")


def selected_packages(root):
    upstream_by_group = {}
    same_index_conflicts = set()
    cross_index_conflicts = set()
    for index in sorted((root / "signed-indexes").rglob("Packages.xz")):
        source_groups = set()
        for raw in lzma.decompress(index.read_bytes()).decode().strip().split("\n\n"):
            item = fields(raw)
            required = ("Package", "Version", "Architecture", "Filename", "SHA256", "Size")
            if not all(key in item for key in required):
                fail("incomplete signed Packages stanza")
            group = tuple(item[key] for key in ("Package", "Version", "Architecture"))
            if group in source_groups:
                same_index_conflicts.add(group)
                continue
            source_groups.add(group)
            if group in upstream_by_group:
                if upstream_by_group[group][0] != raw:
                    cross_index_conflicts.add(group)
            else:
                upstream_by_group[group] = (raw, item)
    result = {}
    for line in (root / "package-manifest.tsv").read_text().splitlines():
        name, version, arch, filename, sha, size = line.split("\t")
        group = (name, version, arch)
        if group in same_index_conflicts:
            fail("duplicate package group in same signed index")
        if group in cross_index_conflicts:
            fail("conflicting signed package metadata across indexes")
        if group not in upstream_by_group:
            fail("selection is not uniquely present in signed Packages")
        item = upstream_by_group[group][1]
        if (
            item.get("Filename"),
            item.get("SHA256"),
            item.get("Size"),
        ) != (filename, sha, size):
            fail("selection is not uniquely present in signed Packages")
        result[group] = item
    return result


def recompute(root, selected):
    by_name = {}
    providers = {}
    for key, item in selected.items():
        by_name.setdefault(key[0], []).append(key)
        for group in apt_pkg.parse_depends(item.get("Provides", ""), False, "arm64"):
            for provided, version, _operator in group:
                providers.setdefault(provided.split(":", 1)[0], []).append((key, version))

    def architecture_eligible(target, candidate, source_arch):
        candidate_arch = candidate["Architecture"]
        multi_arch = candidate.get("Multi-Arch", "no")
        if ":" in target:
            _name, qualifier = target.rsplit(":", 1)
            if qualifier == "native":
                return candidate_arch in ("arm64", "all")
            if qualifier == "any":
                return candidate_arch in ("arm64", "all") and multi_arch == "allowed"
            return candidate_arch in (qualifier, "all")
        return candidate_arch in ("arm64", "all") or multi_arch == "foreign"
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
                        if architecture_eligible(target, candidate, source[2]) and (
                            not operator or apt_pkg.check_dep(key[1], operator, required)
                        ):
                            candidates.append((position, key))
                    for key, provided in providers.get(base, []):
                        candidate = selected[key]
                        if architecture_eligible(target, candidate, source[2]) and (
                            not operator
                            or (provided and apt_pkg.check_dep(provided, operator, required))
                        ):
                            candidates.append((position, key))
                if not candidates:
                    fail("selected set does not satisfy " + expression)
                chosen = min(candidates, key=lambda value: (value[0], value[1]))[1]
                edges.append("\t".join((*requirement, *chosen)))
    return requirements, edges


def bind_roots(root, selected, mode):
    lines = []
    for requested in (root / "roots.txt").read_text().splitlines():
        direct = [(key, item) for key, item in selected.items() if key[0] == requested]
        candidates = direct
        if not candidates and requested in REVIEWED_ROOT_PROVIDERS:
            expected = REVIEWED_ROOT_PROVIDERS[requested]
            candidates = []
            for key, item in selected.items():
                if key[0] != expected:
                    continue
                for group in apt_pkg.parse_depends(item.get("Provides", ""), False, "arm64"):
                    for provided, provided_version, operator in group:
                        if (
                            provided.split(":", 1)[0] == requested
                            and operator == "="
                            and provided_version == key[1]
                        ):
                            candidates.append((key, item))
        if len(candidates) != 1:
            fail("ambiguous requested root binding" if candidates else "missing root package")
        key, _item = candidates[0]
        lines.append("\t".join((requested, *key)))
    canonical(root / "root-bindings.tsv", lines, mode)


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


def verify_roots(root, mode):
    repository = Path(__file__).resolve().parent.parent
    config = tomllib.loads((repository / "images/workspace/bundles/ubuntu-packages.toml").read_text())
    tools_path = repository / config["system_packages_file"]
    if hashlib.sha256(tools_path.read_bytes()).hexdigest() != config["system_packages_sha256"]:
        fail("trusted system package list digest mismatch")
    builder = ["build-essential", "ca-certificates", "git", "libssl-dev", "pkg-config"]
    expected = "\n".join(sorted(set(builder + tools_path.read_text().splitlines()))) + "\n"
    if mode == "--write":
        (root / "roots.txt").write_text(expected)
    elif (root / "roots.txt").read_text() != expected:
        fail("roots differ from trusted builder and system package inputs")


if len(sys.argv) != 3 or sys.argv[1] not in ("--write", "--verify"):
    fail("usage: verify-ubuntu-debian-evidence.py --write|--verify EVIDENCE")
mode, root = sys.argv[1], Path(sys.argv[2])
selected = selected_packages(root)
verify_roots(root, mode)
bind_roots(root, selected, mode)
requirements, edges = recompute(root, selected)
canonical(root / "dependency-requirements.tsv", requirements, mode)
canonical(root / "dependency-edges.tsv", edges, mode)
canonical(root / "offline-apt-check.tsv", offline_check(root, selected), mode)
