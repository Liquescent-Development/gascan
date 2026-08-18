//! OCI layout fixtures, and the engine image store they are loaded into.
//!
//! **Why a crate and not a test module.** Two live tiers need exactly this and
//! neither can import the other's `tests/` tree: `gascan-arca`'s tier drives
//! the engine directly, and `gascan-e2e`'s daemon-on-engine tier drives it
//! through `gascand` and the `gascan` CLI. Both must seed an engine store with
//! an image that stays up, and both must name that image by the digest the
//! STORE recorded rather than the one the layout carried. A second copy of the
//! ustar writer, the gzip framing and the store-digest rule would be a second
//! place for each of those measured details to rot.
//!
//! Everything here panics rather than returning errors. It is fixture code for
//! `#[ignore]`d live tests, and a fixture that fails has nothing to recover to.

use camino::{Utf8Path, Utf8PathBuf};
use std::collections::BTreeMap;

/// A one-image OCI layout that runs `command`, written beside a base layout.
///
/// **`CreateRequest` carries no argv, so this is the only way the tier can
/// decide what a sandbox runs.** `engine.proto`'s `CreateRequest` has no
/// command and no entrypoint field, and `SandboxEngineService` passes
/// `entrypoint: nil, command: nil` deliberately -- the image's own config
/// decides. The environment is no way in either: `policy.rs` sets it from
/// `guest_environment()`, a fixed map with no manifest passthrough. So
/// `gascan-apple`'s `guest_argv` technique does not transfer at all, and the
/// published-port test's responder has to be baked into an image. The port it
/// listens on is therefore known only at image-build time, which is why the
/// image is built during the test rather than prepared by a maintainer.
///
/// This is not an image builder. It reuses the base layout's layers verbatim
/// and writes three small blobs: a config with a new `Cmd`, a manifest naming
/// that config, and an index naming that manifest under `tag`. The rootfs is
/// untouched, so the `diff_ids` still describe it.
///
/// The base layout's `index.json` must name exactly one manifest. Anything
/// else would make "which image is this derived from" a choice this function
/// would have to guess at.
pub fn layout_running(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
) -> Utf8PathBuf {
    layout_running_with_directories(base, destination, tag, command, &[])
}

/// The same, with a layer of its own creating each of `directories`.
///
/// **A mount target that does not exist in the image is not mounted, and
/// nothing says so.** MEASURED against this engine: a sandbox whose three
/// managed volumes target `/home/workspace/.local`, `.cache` and `.config` on a
/// stock alpine rootfs starts successfully, `Inspect` reports it running, and
/// the guest's `/proc/partitions` shows all three block devices attached at
/// exactly their declared sizes -- `vdd` 262144 blocks, `vde` 524288, `vdf`
/// 1048576 -- while `/proc/mounts` lists none of them and `/home` is empty. The
/// engine logs no warning. `/workspace` mounts on the same guest, so the
/// difference is the depth: `/workspace` needs one directory under `/` and
/// `/home/workspace/.local` needs two under an existing `/home`.
///
/// The production workspace image creates all three
/// (`images/workspace/Dockerfile:142-143`), so this is what the tier needs to
/// resemble it. Ancestors are derived rather than demanded from the caller: a
/// list that named a leaf and forgot its parent would reproduce the very
/// failure this exists to remove.
pub fn layout_running_with_directories(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
    directories: &[&str],
) -> Utf8PathBuf {
    let entries: Vec<LayerEntry<'_>> = directories
        .iter()
        .copied()
        .map(LayerEntry::directory)
        .collect();
    layout_running_with_entries(base, destination, tag, command, &entries)
}

/// One path the added layer creates: a directory, or a file with contents.
///
/// **Files are here because a guest can be asked for programs it does not
/// have.** `gascan up` provisions through the guest's own `/usr/bin/sudo`,
/// `/usr/local/bin/select-gascamp` and three siblings -- the workspace image's
/// contract, which P5.4 owns and which a stock base layout does not satisfy.
/// A tier that could only add directories could bring a sandbox up and never
/// provision one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerEntry<'a> {
    pub path: &'a str,
    /// The low twelve bits are what a ustar header records.
    pub mode: u32,
    /// `None` is a directory; `Some` is a regular file with these bytes.
    pub contents: Option<&'a [u8]>,
}

impl<'a> LayerEntry<'a> {
    #[must_use]
    pub const fn directory(path: &'a str) -> Self {
        Self {
            path,
            mode: 0o755,
            contents: None,
        }
    }

    /// An executable regular file, which every guest-side shim here is.
    #[must_use]
    pub const fn program(path: &'a str, contents: &'a [u8]) -> Self {
        Self {
            path,
            mode: 0o755,
            contents: Some(contents),
        }
    }

    /// A world-readable regular file, for `/etc/passwd` and its like.
    #[must_use]
    pub const fn file(path: &'a str, contents: &'a [u8]) -> Self {
        Self {
            path,
            mode: 0o644,
            contents: Some(contents),
        }
    }
}

/// The same, with a layer of its own creating each of `entries`.
pub fn layout_running_with_entries(
    base: &Utf8Path,
    destination: &Utf8Path,
    tag: &str,
    command: &[&str],
    entries: &[LayerEntry<'_>],
) -> Utf8PathBuf {
    use serde_json::{Value, json};

    copy_tree(base, destination);

    let index: Value = read_json(&destination.join("index.json"));
    let manifests = index["manifests"]
        .as_array()
        .unwrap_or_else(|| panic!("{base}/index.json has no manifests array"));
    assert_eq!(
        manifests.len(),
        1,
        "{base}/index.json must name exactly one manifest; it names {}",
        manifests.len()
    );
    let mut manifest: Value = read_json(&blob_path(destination, digest_of(&manifests[0])));
    let mut config: Value = read_json(&blob_path(destination, digest_of(&manifest["config"])));

    // `Entrypoint` is cleared as well as `Cmd` being set. A base image that
    // carried one would prepend it to the command below, and the responder
    // would run as arguments to something else.
    config["config"]["Cmd"] = json!(command);
    config["config"]["Entrypoint"] = Value::Null;

    if !entries.is_empty() {
        // The layer goes on top, and `diff_ids` is appended in the same
        // position: the two lists are parallel by ordinal, so a layer added to
        // one and not the other describes a rootfs the image does not have.
        let archive = layer_archive(entries);
        let diff_id = format!(
            "sha256:{:x}",
            <sha2::Sha256 as sha2::Digest>::digest(&archive)
        );
        let compressed = gzip(&archive);
        let digest = write_bytes(destination, &compressed);
        manifest["layers"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("{base}'s manifest has no layers array"))
            .push(json!({
                "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                "digest": digest,
                "size": compressed.len(),
            }));
        config["rootfs"]["diff_ids"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("{base}'s config has no rootfs.diff_ids array"))
            .push(json!(diff_id));
    }

    let config_blob = write_blob(destination, &config);
    manifest["config"]["digest"] = json!(config_blob.0);
    manifest["config"]["size"] = json!(config_blob.1);
    let manifest_blob = write_blob(destination, &manifest);

    std::fs::write(
        destination.join("index.json"),
        serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{
                "mediaType": manifest["mediaType"],
                "digest": manifest_blob.0,
                "size": manifest_blob.1,
                "annotations": { "org.opencontainers.image.ref.name": tag },
            }],
        }))
        .expect("an index serialises"),
    )
    .unwrap_or_else(|error| panic!("could not write {destination}/index.json: {error}"));
    destination.to_owned()
}

/// A POSIX `ustar` archive holding `entries` and every ancestor directory.
///
/// Written by hand rather than with a crate. The alternative was a `tar`
/// dependency for the one thing this needs from it, or shelling out to the
/// host's `tar` -- which on macOS writes AppleDouble entries into the archive
/// unless told not to, and would put a host tool's defaults inside the image
/// under test.
///
/// Root ownership throughout. The tiers' guests run as root, so nothing here
/// needs finer ownership, and asserting it would be asserting against a
/// fixture. `install -o workspace` inside the guest is what actually sets the
/// ownership the product cares about, and that is the product's step to run.
///
/// **Ancestors are derived rather than demanded from the caller**, for both
/// kinds: a list that named `/usr/local/bin/select-gascamp` and forgot
/// `/usr/local/bin` would write a file into a directory the layer does not
/// create, and an overlay whose parent is absent is exactly the silent
/// non-mount this exists to remove. A directory named explicitly keeps the
/// mode the caller gave it; one derived as an ancestor gets 0755.
fn layer_archive(entries: &[LayerEntry<'_>]) -> Vec<u8> {
    // Directories first and in ancestor order, because `tar` applies entries in
    // the order it finds them and a file cannot precede its own directory.
    let mut directories: Vec<(String, u32)> = Vec::new();
    let mut push_ancestors = |path: &str, own: Option<u32>| {
        let trimmed = path.trim_matches('/');
        let components: Vec<&str> = trimmed.split('/').collect();
        let mut ancestor = String::new();
        let last = components.len().saturating_sub(1);
        for (index, component) in components.iter().enumerate() {
            ancestor.push_str(component);
            ancestor.push('/');
            // `own` is Some only for a directory entry the caller named, and
            // only its final component takes the caller's mode. Every derived
            // ancestor is 0755.
            let mode = if index == last {
                own.unwrap_or(0o755)
            } else {
                0o755
            };
            match directories.iter_mut().find(|(name, _)| *name == ancestor) {
                Some(existing) => {
                    if index == last && own.is_some() {
                        existing.1 = mode;
                    }
                }
                None => directories.push((ancestor.clone(), mode)),
            }
        }
    };
    for entry in entries {
        match entry.contents {
            None => push_ancestors(entry.path, Some(entry.mode)),
            Some(_) => {
                let path = entry.path.trim_matches('/');
                if let Some((parent, _)) = path.rsplit_once('/') {
                    push_ancestors(parent, None);
                }
            }
        }
    }

    let mut archive = Vec::new();
    for (name, mode) in &directories {
        archive.extend_from_slice(&ustar_header(name, *mode, b'5', 0));
    }
    for entry in entries {
        let Some(contents) = entry.contents else {
            continue;
        };
        let name = entry.path.trim_start_matches('/');
        archive.extend_from_slice(&ustar_header(name, entry.mode, b'0', contents.len()));
        archive.extend_from_slice(contents);
        // Every entry's data is padded to a whole 512-byte block, or the next
        // header lands mid-block and the whole archive after it is garbage.
        archive.resize(archive.len().div_ceil(512) * 512, 0);
    }
    // Two zero blocks end the archive, then the whole thing is padded to a
    // 10240-byte record. Both are what `tar` itself writes.
    archive.extend_from_slice(&[0_u8; 1024]);
    archive.resize(archive.len().div_ceil(10240) * 10240, 0);
    archive
}

/// One 512-byte `ustar` header.
///
/// Every numeric field is octal, NUL-terminated, and zero-padded to one less
/// than its width. `chksum` is the exception: it is spaces while the sum is
/// taken, then six digits, a NUL and a space.
fn ustar_header(name: &str, mode: u32, typeflag: u8, size: usize) -> [u8; 512] {
    let mut header = [0_u8; 512];
    assert!(
        name.len() < 100,
        "{name} does not fit a ustar header's 100-byte name field"
    );
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(format!("{:07o}\0", mode & 0o7777).as_bytes());
    header[108..116].copy_from_slice(b"0000000\0"); // uid
    header[116..124].copy_from_slice(b"0000000\0"); // gid
    header[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0"); // mtime
    header[148..156].copy_from_slice(b"        "); // chksum, while summing
    header[156] = typeflag;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    header[265..269].copy_from_slice(b"root");
    header[297..301].copy_from_slice(b"root");
    let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    let checksum = format!("{sum:06o}\0 ");
    header[148..156].copy_from_slice(checksum.as_bytes());
    header
}

/// `data` in gzip form, with every deflate block stored rather than compressed.
///
/// A stored block is the one deflate encoding that needs no compressor: five
/// bytes of header and the bytes themselves. The layer this wraps is a few
/// kilobytes of mostly zeroes, so the size costs nothing, and the media type
/// the manifest declares is the same `tar+gzip` the base layout's own layer
/// carries -- the unpacker takes exactly the path it already takes.
fn gzip(data: &[u8]) -> Vec<u8> {
    // Magic, deflate, no flags, no mtime, no extra flags, unix.
    let mut out = vec![0x1f, 0x8b, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0x03];
    let mut chunks = data.chunks(0xffff).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let length = u16::try_from(chunk.len()).expect("a chunk is at most 0xffff bytes");
        out.push(u8::from(chunks.peek().is_none())); // BFINAL, BTYPE = stored
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&(!length).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    #[expect(
        clippy::cast_possible_truncation,
        reason = "gzip's ISIZE is defined as the length modulo 2^32"
    )]
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// The CRC-32 gzip's trailer carries, computed a bit at a time.
///
/// No table: this runs over a few kilobytes once per test, and a table would be
/// a second thing to get right.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn digest_of(descriptor: &serde_json::Value) -> &str {
    descriptor["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("an OCI descriptor with no digest: {descriptor}"))
}

fn blob_path(layout: &Utf8Path, digest: &str) -> Utf8PathBuf {
    let (algorithm, hex) = digest
        .split_once(':')
        .unwrap_or_else(|| panic!("{digest} is not an OCI digest"));
    layout.join("blobs").join(algorithm).join(hex)
}

fn read_json(path: &Utf8Path) -> serde_json::Value {
    let source =
        std::fs::read(path).unwrap_or_else(|error| panic!("could not read {path}: {error}"));
    serde_json::from_slice(&source)
        .unwrap_or_else(|error| panic!("could not parse {path} as json: {error}"))
}

/// Writes `value` as a content-addressed blob, returning its digest and size.
///
/// The bytes that are hashed are the bytes that are written -- one
/// serialisation, used for both -- because a digest taken over a second
/// rendering would name content the layout does not contain, and the engine
/// verifies blobs it loads.
fn write_blob(layout: &Utf8Path, value: &serde_json::Value) -> (String, usize) {
    let bytes = serde_json::to_vec(value).expect("a blob serialises");
    let digest = write_bytes(layout, &bytes);
    (digest, bytes.len())
}

/// Writes `bytes` under their own digest, and returns it.
fn write_bytes(layout: &Utf8Path, bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    let digest = format!("sha256:{:x}", Sha256::digest(bytes));
    let path = blob_path(layout, &digest);
    std::fs::write(&path, bytes).unwrap_or_else(|error| panic!("could not write {path}: {error}"));
    digest
}

fn copy_tree(from: &Utf8Path, to: &Utf8Path) {
    std::fs::create_dir_all(to).unwrap_or_else(|error| panic!("could not create {to}: {error}"));
    let entries = std::fs::read_dir(from)
        .unwrap_or_else(|error| panic!("could not read the base layout {from}: {error}"));
    for entry in entries {
        let entry = entry.expect("a directory entry");
        let name = entry.file_name();
        let name = name.to_str().expect("a utf-8 layout entry name");
        let source = from.join(name);
        let target = to.join(name);
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::copy(&source, &target)
                .unwrap_or_else(|error| panic!("could not copy {source} to {target}: {error}"));
        }
    }
}

/// Seeds one OCI layout into an engine state root, before any engine serves it.
///
/// Failure is a panic carrying the subcommand's own output: a test whose store
/// is empty fails later as a `not_found` from `Create`, which reads as an
/// engine defect and is not one.
///
/// Blocking, and called from `async` test bodies without complaint. `image
/// load` binds no socket, starts no VM and needs no kernel; it runs to
/// completion and exits, so there is no concurrency for a runtime to overlap it
/// with. An `async` variant would oblige every caller to have a runtime for a
/// process that finishes before anything else could be scheduled.
pub fn load_image(binary: &str, state: &Utf8Path, layout: &Utf8Path) {
    let output = std::process::Command::new(binary)
        .arg("image")
        .arg("load")
        .arg("--state-root")
        .arg(state.as_str())
        .arg("--oci-layout")
        .arg(layout.as_str())
        .output()
        .unwrap_or_else(|error| panic!("could not run {binary} image load: {error}"));
    assert!(
        output.status.success(),
        "{binary} image load --state-root {state} --oci-layout {layout} exited with {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Every tag the engine's own image store records, mapped to its digest.
///
/// Read from the store rather than from the layout, for the reason
/// [`stored_image_reference`] records. An absent file is an empty store, which
/// is what an engine started with no layouts has.
#[must_use]
pub fn stored_images(state: &Utf8Path) -> BTreeMap<String, String> {
    let path = state.join("images").join("state.json");
    let Ok(source) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(&source)
        .unwrap_or_else(|error| panic!("could not parse the engine's image store {path}: {error}"));
    parsed
        .into_iter()
        .map(|(tag, descriptor)| {
            let digest = descriptor
                .get("digest")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("{path} records {tag} with no digest: {descriptor}"))
                .to_owned();
            (tag, digest)
        })
        .collect()
}

/// The immutable reference naming what `images` holds under `tag`.
///
/// **THE DIGEST A REQUEST MUST NAME IS THE STORE'S, NOT THE LAYOUT'S.** The
/// store re-wraps what it ingests: a layout whose `index.json` carries manifest
/// `sha256:45e09956…` is recorded in `<state-root>/images/state.json` as an
/// image *index* under `sha256:a019d0ba…`. A test that derived the digest from
/// the layout it loaded would name content the engine does not hold, and hear
/// `not_found` from a store that has the image.
#[must_use]
pub fn stored_image_reference(images: &BTreeMap<String, String>, tag: &str) -> String {
    let digest = images.get(tag).unwrap_or_else(|| {
        panic!(
            "the engine's store holds no image tagged {tag}; it holds {:?}",
            images.keys().collect::<Vec<_>>()
        )
    });
    format!("{}@{digest}", repository_of(tag))
}

/// The repository half of a reference, split the way both sides of the wire do.
///
/// The rule is `immutable_image_identity`'s
/// (`crates/gascan-core/src/runtime.rs`), mirrored by Arca's
/// `ImageIdentity.repository(of:)`: drop anything from `@sha256:` onward, then
/// drop a tag -- the last `:` that comes *after* the last `/`, so the port in
/// `registry.example:5000/repo` is not mistaken for one. `heldImageReferences`
/// compares the request's repository against the store's, so a split that
/// disagreed with Arca's would be refused as `not_found` for content the
/// engine holds.
#[must_use]
pub fn repository_of(reference: &str) -> &str {
    let reference = reference.split_once("@sha256:").map_or(reference, |a| a.0);
    match reference.rfind(':') {
        Some(colon) if !reference[colon..].contains('/') => &reference[..colon],
        _ => reference,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The archive is read back by the system `tar`, not by this module.**
    ///
    /// A hand-written ustar writer checked by a hand-written reader would agree
    /// with itself about a checksum field that is wrong, and the reader that
    /// matters is the one inside the guest's overlay unpacker. `/usr/bin/tar`
    /// is the closest cheap stand-in for it.
    ///
    /// **The padding mutation leaves `tar` exiting 0**, which is why this
    /// asserts on what landed on disk and not on the status. MEASURED, by
    /// deleting the `resize` that pads each entry's data to a 512-byte block:
    /// `tar` succeeds, extracts `/usr/local/bin/select-gascamp`, and silently
    /// drops `/etc/passwd` -- the next header landed mid-block. A test that
    /// checked only `status.success()` would have passed.
    ///
    /// **What this test does NOT defend, stated because it looks as if it
    /// does:** deriving a file's ancestor directories. MEASURED, with that
    /// derivation deleted, this test still passes -- `tar` creates a missing
    /// parent implicitly. The derivation is kept for the reader that matters,
    /// the guest's overlay unpacker, which this tier cannot reach from a unit
    /// test. It is reasoning, not a tested property.
    #[test]
    fn the_layer_archive_is_a_tar_the_system_tool_can_extract() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = camino::Utf8Path::from_path(directory.path()).expect("a utf-8 path");

        let archive = layer_archive(&[
            LayerEntry::directory("/home/workspace/.cache"),
            LayerEntry::program("/usr/local/bin/select-gascamp", b"#!/bin/sh\necho '{}'\n"),
            LayerEntry::file("/etc/passwd", b"root:x:0:0:root:/root:/bin/sh\n"),
        ]);
        let path = root.join("layer.tar");
        std::fs::write(&path, &archive).expect("the archive is written");

        let extracted = root.join("out");
        std::fs::create_dir(&extracted).expect("an extraction root");
        let output = std::process::Command::new("/usr/bin/tar")
            .arg("-xf")
            .arg(path.as_str())
            .arg("-C")
            .arg(extracted.as_str())
            .output()
            .expect("tar runs");
        assert!(
            output.status.success(),
            "tar refused the archive: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // The derived ancestors exist, which is the half a caller never names.
        assert!(extracted.join("home").is_dir());
        assert!(extracted.join("home/workspace").is_dir());
        assert!(extracted.join("home/workspace/.cache").is_dir());
        assert!(extracted.join("usr/local/bin").is_dir());

        assert_eq!(
            std::fs::read(extracted.join("usr/local/bin/select-gascamp"))
                .expect("the program is extracted"),
            b"#!/bin/sh\necho '{}'\n"
        );
        assert_eq!(
            std::fs::read(extracted.join("etc/passwd")).expect("the file is extracted"),
            b"root:x:0:0:root:/root:/bin/sh\n"
        );

        // The mode is what makes a shim runnable, and tar carries it.
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(extracted.join("usr/local/bin/select-gascamp"))
            .expect("the program is present")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o755,
            "a shim that is not executable cannot provision"
        );
    }
}
