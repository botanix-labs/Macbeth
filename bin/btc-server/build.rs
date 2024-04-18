fn main() {
    let protos = &["proto/btc_server.proto"];

    // server
    let prost_config_server = prost_build::Config::default();
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("src/rpc")
        .compile_with_config(prost_config_server, protos, &[] as &[&str])
        .expect("failed to compile server protos");

    // client
    let prost_config_client = prost_build::Config::default();
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("client/src/")
        .compile_with_config(prost_config_client, protos, &[] as &[&str])
        .expect("failed to compile client protos");
}
