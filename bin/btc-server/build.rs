fn main() {
    let protos = &["proto/btc_server.proto"];

    // server
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .build_transport(true)
        //.file_descriptor_set_path("src/proto/api.bin")
        .out_dir("src/rpc")
        .compile(protos, &[] as &[&str])
        .expect("failed to compile server protos");

    // client
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .build_transport(true)
        //.file_descriptor_set_path("src/proto/api.bin")
        .out_dir("client/src/")
        .compile(protos, &[] as &[&str])
        .expect("failed to compile client protos");
}
