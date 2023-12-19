fn main() {
    let protos = &["proto/btc_server.proto"];

    // server
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .out_dir("src/rpc")
        .compile(protos, &[] as &[&str])
        .expect("failed to compile server protos");

    // client
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .out_dir("client/src/")
        .compile(protos, &[] as &[&str])
        .expect("failed to compile client protos");
}
