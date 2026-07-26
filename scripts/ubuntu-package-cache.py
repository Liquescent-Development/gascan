#!/usr/bin/env python3
import hashlib
import lzma
import os
import shutil
import subprocess
import sys
import urllib.parse
import uuid
from pathlib import Path


def fail(message):
    raise SystemExit("ubuntu package cache: " + message)


def parse_fields(raw):
    result = {}
    current = None
    for line in raw.splitlines():
        if line.startswith((" ", "\t")):
            if current is None:
                fail("invalid signed Packages continuation")
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


def signed_records(evidence):
    records = {}
    same_index_conflicts = set()
    cross_index_conflicts = set()
    indexes = sorted((evidence / "signed-indexes").rglob("Packages.xz"))
    if not indexes:
        fail("signed Packages indexes are missing")
    for index in indexes:
        source_groups = set()
        try:
            text = lzma.decompress(index.read_bytes()).decode("utf-8", "strict")
        except (lzma.LZMAError, UnicodeDecodeError):
            fail("invalid signed Packages index")
        for raw in text.strip().split("\n\n"):
            item = parse_fields(raw)
            required = ("Package", "Version", "Architecture", "Filename", "SHA256", "Size")
            if not all(item.get(field) for field in required):
                fail("incomplete signed Packages stanza")
            group = tuple(item[field] for field in ("Package", "Version", "Architecture"))
            if group in source_groups:
                same_index_conflicts.add(group)
                continue
            source_groups.add(group)
            if group in records:
                if records[group][0] != raw:
                    cross_index_conflicts.add(group)
            else:
                records[group] = (raw, item)
    return (
        {group: record[1] for group, record in records.items()},
        same_index_conflicts,
        cross_index_conflicts,
    )


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def package_tuple(path):
    dpkg_deb = os.environ.get("DPKG_DEB", "dpkg-deb")
    result = subprocess.run(
        [
            dpkg_deb,
            "--show",
            "--showformat=${Package}\t${Version}\t${Architecture}\n",
            str(path),
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if result.returncode:
        return None
    raw = result.stdout
    if raw.count("\n") != 1:
        return None
    columns = raw.removesuffix("\n").split("\t")
    if (
        len(columns) != 3
        or not all(columns)
        or any(
            ord(character) < 32 or ord(character) == 127
            for column in columns
            for character in column
        )
    ):
        return None
    return tuple(columns)


def validate(path, records, same_index_conflicts, cross_index_conflicts):
    group = package_tuple(path)
    if group is None or group not in records:
        return None
    if group in same_index_conflicts:
        fail("duplicate package group in same signed index")
    if group in cross_index_conflicts:
        fail("conflicting signed package metadata across indexes")
    record = records[group]
    decoded_name = urllib.parse.unquote(path.name)
    expected_cache_name = f"{group[0]}_{group[1]}_{group[2]}.deb"
    if decoded_name != expected_cache_name or not Path(record["Filename"]).name:
        return None
    try:
        expected_size = int(record["Size"])
    except ValueError:
        fail("invalid signed package size")
    if path.stat().st_size != expected_size or digest(path) != record["SHA256"]:
        return None
    return record


def atomic_copy(source, destination, interrupt):
    temporary = destination.with_name(
        "." + destination.name + ".tmp-" + uuid.uuid4().hex
    )
    try:
        with source.open("rb") as incoming, temporary.open("xb") as outgoing:
            shutil.copyfileobj(incoming, outgoing)
            outgoing.flush()
            os.fsync(outgoing.fileno())
        if interrupt:
            fail("injected atomic publication interruption")
        os.replace(temporary, destination)
        directory = os.open(destination.parent, os.O_RDONLY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        temporary.unlink(missing_ok=True)


def stage(records, same_index_conflicts, cross_index_conflicts, shared, private):
    private.mkdir(parents=True, exist_ok=True)
    for candidate in sorted(shared.glob("*.deb")):
        if validate(candidate, records, same_index_conflicts, cross_index_conflicts) is None:
            continue
        atomic_copy(candidate, private / candidate.name, False)


def publish(records, same_index_conflicts, cross_index_conflicts, shared, private):
    candidates = sorted(private.glob("*.deb"))
    validated = []
    for candidate in candidates:
        if validate(candidate, records, same_index_conflicts, cross_index_conflicts) is None:
            fail(
                "private package payload is not bound to signed metadata: "
                + candidate.name
            )
        validated.append(candidate)
    shared.mkdir(parents=True, exist_ok=True)
    interrupt = os.environ.get("UBUNTU_CACHE_INTERRUPT_AFTER_COPY") == "1"
    for index, candidate in enumerate(validated):
        destination = shared / candidate.name
        if (
            destination.is_file()
            and validate(
                destination, records, same_index_conflicts, cross_index_conflicts
            )
            is not None
        ):
            continue
        atomic_copy(candidate, destination, interrupt and index == 0)


if len(sys.argv) != 5 or sys.argv[1] not in ("stage", "publish"):
    fail("usage: ubuntu-package-cache.py stage|publish EVIDENCE SHARED PRIVATE")

mode = sys.argv[1]
evidence, shared, private = map(Path, sys.argv[2:])
records, same_index_conflicts, cross_index_conflicts = signed_records(evidence)
if mode == "stage":
    stage(records, same_index_conflicts, cross_index_conflicts, shared, private)
else:
    publish(records, same_index_conflicts, cross_index_conflicts, shared, private)
