use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let mut prost = prost_build::Config::new();
    prost.protoc_executable(protoc);
    tonic_build::configure()
        .file_descriptor_set_path(out_dir.join("gascan_descriptor.bin"))
        .compile_protos_with_config(
            prost,
            &["../../proto/gascan/v1/gascan.proto"],
            &["../../proto"],
        )?;
    println!("cargo:rerun-if-changed=../../proto/gascan/v1/gascan.proto");
    Ok(())
}
