use std::{
    fs::File,
    io::{BufReader, BufWriter, Cursor, Read, Write},
};

use faces_quic_client::*;
use log::{debug, error, info, warn};

fn main() {
    env_logger::init();

    let peer = "192.168.1.22:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let progress_tracker = ProgressTracker::new();

    progress_tracker.run(|event| match event {
        RequestEvent::UploadProgress(progress) => {
            info!("upload[{}]", progress.progress());
        }
        RequestEvent::DownloadProgress(progress) => {
            info!(
                "[{:?}] [{}] download = [{}]",
                progress.req_path(),
                progress.uuid(),
                progress.progress()
            );
        }
        RequestEvent::ConnexionClosed(scid) => {}
    });

    let res = client
        .get("/test_mini")
        .set_user_agent("camille_2")
        .subscribe_event(progress_tracker.clone())
        .send()
        .unwrap();

    let res_2 = client
        .post_file("/large_data", "/home/camille/llvm.sh")
        .set_user_agent("camille_2")
        .subscribe_event(progress_tracker.clone())
        .send()
        .unwrap();
    let res_3 = client
        .post_data(
            "/large_data",
            b"Hi it-s Coop, and I like coffee. But, Do you know where I can find Judy ?".to_vec(),
        )
        .set_user_agent("camille_2")
        .subscribe_event(progress_tracker.clone())
        .send()
        .unwrap();

    // let res = res.wait_response();

    //warn!("recv [{}]", res.unwrap().take_data().len());
    let res_1 = res.wait_response();
    let res_2 = res_2.wait_response();

    warn!("recv [{}]", res_2.unwrap().take_data().len());
}
