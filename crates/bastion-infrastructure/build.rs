fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_path = std::path::PathBuf::from("../../proto/sandbox/v1/sandbox.proto");

    if proto_path.exists() {
        tonic_prost_build::configure()
            .build_server(false)
            .build_client(true)
            .compile_protos(
                &[proto_path.to_str().unwrap()],
                &["../../proto"],
            )?;
    }

    Ok(())
}
