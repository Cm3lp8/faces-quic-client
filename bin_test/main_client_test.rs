use faces_quic_client::*;
use log::{debug, info};

fn main() {
    env_logger::init();

    let peer = "127.0.0.1:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let res = client
        .new_request(|req| {
            req.post("/large_data", vec![22; 180_700_000], BodyType::Array)
                .set_user_agent("camille");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));
    let res_2 = client
        .new_request(|req| {
            req.post("/large_data", vec![2; 120_700_000], BodyType::Array)
                .set_user_agent("camille");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some progress2 [{:?}]", progress.progress()));

    let mut res = res.wait_response().unwrap();
    let mut res_2 = res_2.wait_response().unwrap();

    let data = res.take_data();
    let data_len = data.len();

    println!("[{}]", res);

    println!("[{:?}] [{}]", &data[data_len - 5..], data_len);
    let data = res_2.take_data();
    let data_len = data.len();

    println!("[{}]", res_2);

    println!("[{:?}] [{}]", &data[data_len - 5..], data_len);
}
