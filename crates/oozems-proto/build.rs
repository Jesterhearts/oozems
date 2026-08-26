fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let descriptor_path =
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("oozems_descriptor.bin");
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.file_descriptor_set_path(descriptor_path);
    config.compile_protos(&["proto/oozems.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/oozems.proto");
    Ok(())
}
