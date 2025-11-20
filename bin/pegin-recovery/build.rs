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

    // client
    let prost_config_client = prost_build::Config::default();
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/pegin_recovery.bin")
        .out_dir("client/src/")
        .compile_protos_with_config(prost_config_client, protos, &[] as &[&str])
        .expect("failed to compile client protos");

    Ok(())
}
