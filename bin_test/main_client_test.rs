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
            info!("download = [{}]", progress.progress());
        }
    });

    let res = client
        .new_request(|req| {
            req.post_data("/large_data", vec![8; 60_000_000])
                .set_user_agent("camille_0");
            req.subscribe_event(progress_tracker);
        })
        .unwrap();

    let res = res.wait_response();

    warn!("recv [{}]", res.unwrap().take_data().len());
}
