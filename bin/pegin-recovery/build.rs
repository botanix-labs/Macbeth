use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let protos = &["proto/pegin_recovery.proto"];

    // Compile server protos
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/pegin_recovery.bin")
        .out_dir("src/rpc")
        .compile_protos(protos, &[] as &[&str])
        .expect("failed to compile server protos");

    Ok(())
}
