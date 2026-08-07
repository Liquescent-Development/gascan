use std::{
    env,
    error::Error,
    path::{Path, PathBuf},
    process::Command,
};

/// Generate the Arca sandbox-engine client from the proto at the pinned Arca
/// revision.
///
/// The proto is not in this repository and is deliberately not copied into it: a
/// second copy of a published contract is a copy that drifts, and the drift would
/// only be visible to whoever thought to compare them. Instead the file is taken
/// from the signed pin at build time.
///
/// `scripts/sync-arca-proto.sh` owns fetching and verifying it, and prints the
/// directory it landed in. Calling it here rather than reimplementing the pin
/// logic keeps one definition of what "the pinned contract" means, and means this
/// file never has to parse the pin. The script short-circuits on a warm cache, so
/// the network cost is paid once per pin bump rather than once per build.
fn main() -> Result<(), Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("crate is not two levels below the repository root")?;
    let sync_script = repo_root.join("scripts/sync-arca-proto.sh");
    let pin_file = repo_root.join("engine/arca-pin.json");

    // Only these two inputs. Naming any rerun-if-changed replaces cargo's default
    // of watching the whole package, which is what we want: the proto lives
    // outside this package, and the pin is the thing that decides which proto.
    println!("cargo:rerun-if-changed={}", sync_script.display());
    println!("cargo:rerun-if-changed={}", pin_file.display());

    let sync = Command::new(&sync_script)
        .output()
        .map_err(|error| format!("failed to run {}: {error}", sync_script.display()))?;
    if !sync.status.success() {
        // The script's diagnostics are the useful part -- which pin, which
        // revision, which assertion failed -- so they are forwarded rather than
        // replaced with a summary of them.
        return Err(format!(
            "{} failed with {}\n{}",
            sync_script.display(),
            sync.status,
            String::from_utf8_lossy(&sync.stderr).trim_end(),
        )
        .into());
    }

    let extract = PathBuf::from(String::from_utf8(sync.stdout)?.trim());
    let include_root = extract.join("proto");
    let proto = include_root.join("arca/engine/v1/engine.proto");
    if !proto.is_file() {
        return Err(format!(
            "{} reported {} but no engine proto is there",
            sync_script.display(),
            extract.display()
        )
        .into());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut prost = prost_build::Config::new();
    prost.protoc_executable(protoc);
    tonic_build::configure()
        // Gas Can is this contract's client. Arca serves it, from the Swift
        // server code generated in Arca's own tree, so a Rust server here would
        // be surface with no implementor and no caller.
        .build_server(false)
        .file_descriptor_set_path(out_dir.join("arca_engine_descriptor.bin"))
        .compile_protos_with_config(prost, &[&proto], &[&include_root])?;
    Ok(())
}
