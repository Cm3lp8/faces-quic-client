use std::{
    fs::File,
    io::{BufReader, BufWriter, Cursor, Read, Write},
};

use faces_quic_client::*;
use log::{debug, error, info};

fn main() {
    env_logger::init();

    let peer = "192.168.1.22:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let res = client
        .new_request(|req| {
            req.get("/test").set_user_agent("camille_0");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));

    let res_2 = client
        .new_request(|req| {
            req.post_data("/large_data", vec![1; 50_000_000])
                .set_user_agent("camille_1");
        })
        .unwrap()
        .with_progress_callback(|progress| info!("some progress2 [{:?}]", progress.progress()));

    /*
        let res_3 = client
            .new_request(|req| {
                req.post_file("/large_data", "/home/camille/v_info.txt")
                    .set_content_type(ContentType::TextPlain)
                    .set_user_agent("camille_1");
            })
            .unwrap()
            .with_progress_callback(|progress| info!("some proress [{:?}]", progress.progress()));

        let res_4 = client
            .new_request(|req| {
                req.post_file("/large_data", "/home/camille/v_info.txt")
                    .set_content_type(ContentType::TextPlain)
                    .set_user_agent("camille_1");
            })
            .unwrap()
            .with_progress_callback(|progress| info!("some progress2 [{:?}]", progress.progress()));
    */
    // let data = res_2.take_data();
    // let data_len = data.len();
    let mut res = res.wait_response().unwrap();

    let data = res.take_data();
    let data_len = data.len();
    // println!("stream_ 1 data len [{}]", data_len);
    let path = "/home/camille/Vidéos/vid_test_reception.mp4";

    let file = File::create(path).unwrap();
    let mut buf_writer = BufWriter::new(file);

    let mut buf_reader = BufReader::new(Cursor::new(data));

    let mut buf = [0; 4096];
    let mut byte_written = 0;
    while let Ok(n) = buf_reader.read(&mut buf) {
        match buf_writer.write(&buf[..n]) {
            Ok(n) => {
                byte_written += n;
            }
            Err(_e) => error!("can't write file"),
        }
        if byte_written == data_len {
            break;
        }
    }
    buf_writer.flush();

    let mut res_2 = res_2.wait_response().unwrap();
    let data = res_2.take_data();
    let data_len = data.len();
    // let mut res_3 = res_3.wait_response().unwrap();
    //let mut res_4 = res_4.wait_response().unwrap();

    let path = "/home/camille/Vidéos/vid_test_reception_2.mp4";

    let file = File::create(path).unwrap();
    let mut buf_writer = BufWriter::new(file);

    let mut buf_reader = BufReader::new(Cursor::new(data));

    let mut buf = [0; 4096];
    let mut byte_written = 0;
    while let Ok(n) = buf_reader.read(&mut buf) {
        match buf_writer.write(&buf[..n]) {
            Ok(n) => {
                byte_written += n;
            }
            Err(_e) => error!("can't write file"),
        }
        if byte_written == data_len {
            break;
        }
    }
    buf_writer.flush();
    println!(" [{}]", res_2);

    println!("data len [{}]stream_0", data_len);

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
