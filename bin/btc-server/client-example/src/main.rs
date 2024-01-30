use client::{BtcServerClient, Empty};
use tonic::Request;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = BtcServerClient::connect("http://localhost:8080").await.unwrap();
    let request = Request::new(Empty {}); // Use the generated Empty struct

    let response_pk = client.get_public_key(request).await.unwrap();
    println!("Public Key: {}", response_pk.get_ref().publickey);

    // let response_getround1_dkg = client.


    Ok(())
}
