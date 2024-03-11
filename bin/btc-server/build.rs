fn main() {
    let protos = &["proto/btc_server.proto"];

    // server
    let mut prost_config_server = prost_build::Config::default();
    prost_config_server.protoc_arg("--experimental_allow_proto3_optional");
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("src/rpc")
        .compile_with_config(prost_config_server, protos, &[] as &[&str])
        .expect("failed to compile server protos");

    // client
    let mut prost_config_client = prost_build::Config::default();
    prost_config_client.protoc_arg("--experimental_allow_proto3_optional");
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("client/src/")
        .compile_with_config(prost_config_client, protos, &[] as &[&str])
        .expect("failed to compile client protos");
}
