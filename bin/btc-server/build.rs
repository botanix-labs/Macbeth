use std::{env, error::Error};
use vergen::EmitBuilder;

fn main() -> Result<(), Box<dyn Error>> {
    let protos = &["proto/btc_server.proto"];

    // server
    let prost_config_server = prost_build::Config::default();
    tonic_build::configure()
        .build_server(true)
        .build_client(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("src/rpc")
        .compile_protos_with_config(prost_config_server, protos, &[] as &[&str])
        .expect("failed to compile server protos");

    // client
    let prost_config_client = prost_build::Config::default();
    tonic_build::configure()
        .build_client(true)
        .build_server(false)
        .build_transport(true)
        .file_descriptor_set_path("src/rpc/btc_server.bin")
        .out_dir("client/src/")
        .compile_protos_with_config(prost_config_client, protos, &[] as &[&str])
        .expect("failed to compile client protos");

    // emit the instructions
    EmitBuilder::builder()
        .all_rustc()
        .git_describe(false, true, None)
        .git_dirty(true)
        .git_sha(false)
        .build_timestamp()
        .cargo_features()
        .cargo_target_triple()
        .emit_and_set()?;

    let sha = env::var("VERGEN_GIT_SHA")?;
    let sha_short = &sha[0..7];

    let is_dirty = env::var("VERGEN_GIT_DIRTY")? == "true";
    // > git describe --always --tags
    // if not on a tag: v0.2.0-beta.3-82-g1939939b
    // if on a tag: v0.2.0-beta.3
    let not_on_tag = env::var("VERGEN_GIT_DESCRIBE")?.ends_with(&format!("-g{sha_short}"));
    let is_dev = is_dirty || not_on_tag;
    println!("cargo:rustc-env=BTCSERVER_VERSION_SUFFIX={}", if is_dev { "-dev" } else { "" });

    Ok(())
}
