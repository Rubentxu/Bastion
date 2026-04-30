fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_path = std::path::PathBuf::from("../../proto/sandbox/v1/sandbox.proto");
    if proto_path.exists() {
        let path_str = proto_path.to_string_lossy();
        tonic_prost_build::configure()
            .build_server(true)
            .build_client(false)
            .compile_protos(&[&*path_str], &["../../proto"])?;
    }
    Ok(())
}