use faces_quic_client::*;
use log::info;

fn main() {
    env_logger::init();

    let peer = "127.0.0.1:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let res = client
        .new_request(|req| {
            req.post("/large_data", vec![22; 1000000_000], BodyType::Array)
                .set_user_agent("camille");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));

    let res = res.wait_response().unwrap();

    println!("[{}]", res);
}
