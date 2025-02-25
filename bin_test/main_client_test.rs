use faces_quic_client::*;

fn main() {
    let client_config = ClientConfig::new();

    println!("Create config...");
    client_config
        .connexion_infos()
        .set_peer_address("127.0.0.1:3000")
        .set_local_address("0.0.0.0:0")
        .build_connexion_infos();

    println!("Connect to 127.0.0.1:3000");

    let http3_client_manager = Http3ClientManager::new(client_config);

    let req_manager = http3_client_manager.request_manager();

    /*
        let response_0 = req_manager
            .new_request(|req_builder| {
                req_builder
                    .set_method(H3Method::GET)
                    .set_path("/")
                    .set_user_agent("Camille");
            })
            .unwrap();

        let mut res = response_0.wait_response().unwrap();

        println!("[{}]", res);

        let data = res.take_data();

        println!("[{}]", String::from_utf8_lossy(&data));
        println!("Second req");
    */
    let data = vec![0, 1, 2, 3, 4];
    let body_type = BodyType::Array;
    let response_1 = req_manager
        .new_request(|req_builder| {
            req_builder
                .set_method(H3Method::POST { data, body_type })
                .set_path("/large_data")
                .set_user_agent("Cmlp");
        })
        .unwrap();

    let mut res = response_1.wait_response().unwrap();

    println!("[{}]", res);

    let data = res.take_data();

    println!("rec [{:?}]", String::from_utf8_lossy(&data[..10]));
}
