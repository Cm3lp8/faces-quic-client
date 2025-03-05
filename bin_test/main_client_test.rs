use faces_quic_client::*;
use log::{debug, info};

fn main() {
    env_logger::init();

    let peer = "192.168.1.22:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let res = client
        .new_request(|req| {
            req.post_file("/large_data", "/home/camille/curl2.sh")
                .set_content_type(ContentType::TextPlain)
                .set_user_agent("camille_0");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));

    let res_2 = client
        .new_request(|req| {
            req.post_file("/large_data", "/home/camille/v_info.txt")
                .set_content_type(ContentType::TextPlain)
                .set_user_agent("camille_1");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some progress2 [{:?}]", progress.progress()));
    /*
        let res_3 = client
            .new_request(|req| {
                req.post("/large_data", vec![22; 50_000_000], BodyType::Array)
                    .set_user_agent("camille_2");
            })
            .unwrap()
            .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));

        let res_4 = client
            .new_request(|req| {
                req.post("/large_data", vec![2; 50_000_000], BodyType::Array)
                    .set_user_agent("camille_3");
            })
            .unwrap()
            .with_progress_callback(|progress| info!("some progress2 [{:?}]", progress.progress()));
    */
    let mut res = res.wait_response().unwrap();
    let mut res_2 = res_2.wait_response().unwrap();
    // let mut res_3 = res_3.wait_response().unwrap();
    //let mut res_4 = res_4.wait_response().unwrap();

    let data = res.take_data();
    let data_len = data.len();

    println!("[{}]", res);

    println!("[{:?}] [{}]", &data[data_len - 5..], data_len);

    let data = res_2.take_data();
    let data_len = data.len();

    println!("[{}]", res_2);

    println!("[{:?}] [{}]", &data[data_len - 5..], data_len);
    /*
        let data = res_3.take_data();
        let data_len = data.len();

        println!("[{}]", res_3);

        println!("[{:?}] [{}]", &data[data_len - 5..], data_len);
        let data = res_4.take_data();
        let data_len = data.len();

        println!("[{}]", res_4);

        println!("[{:?}] [{}]", &data[data_len - 5..], data_len);
    */
}
