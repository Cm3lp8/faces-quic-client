use std::{
    fs::File,
    io::{BufReader, BufWriter, Cursor, Read, Write},
};

use faces_quic_client::*;
use log::{debug, error, info, warn};

use crate::{
    create_user_implementation::{NewUser, UserCreated},
    other_json_struct::RequestError,
};

fn main() {
    env_logger::init();

    let peer = "192.168.1.22:3000";
    println!("Connect to [{}]", peer);

    let client = Http3ClientManager::new(peer);

    let progress_tracker = ProgressTracker::new();
    let progress_tracker_2 = ProgressTracker::new();
    let progress_tracker_3 = ProgressTracker::new();
    let progress_tracker_4 = ProgressTracker::new();

    progress_tracker.run(|event| match event {
        RequestEvent::UploadProgress(progress) => {
            warn!("upload1[{}]", progress.progress());
        }
        RequestEvent::DownloadProgress(progress) => {
            warn!(
                "[{:?}] [{}] download = [{}]",
                progress.req_path(),
                progress.uuid(),
                progress.progress()
            );
        }
        RequestEvent::ConnexionClosed(scid) => {}
    });

    let new_user = NewUser::create("Camille", 36, "1234");

    let res_2 = client
        .post_data("/create_user", new_user)
        .set_user_agent("camille_2")
        .header("x-name", "Cm3lp8")
        .subscribe_event(progress_tracker_3.clone())
        .send()
        .unwrap();

    let mut res_2 = res_2.wait_response().unwrap();
    //let res_3 = res_3.wait_response();

    info!("recv [{:?}]", res_2.headers());

    match res_2.status() {
        ReqStatus::Success {
            stream_id,
            headers,
            data,
        } => {
            if let Some(data) = &data {
                info!(
                    "succes !! [{:#?}]",
                    serde_json::from_slice::<UserCreated>(data)
                );
            }
        }
        ReqStatus::Error {
            stream_id,
            headers,
            data,
        } => {
            if let Some(error) = &data {
                error!("[{:#?}]", serde_json::from_slice::<RequestError>(error))
            }
        }
        ReqStatus::None => {}
    }
}

mod other_json_struct {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug)]
    pub struct RequestError {
        error: String,
    }
}

mod create_user_implementation {
    use serde::{Deserialize, Serialize};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    #[derive(Serialize, Deserialize, Debug)]
    pub struct UserCreated {
        name: String,
        age: u8,
    }

    #[derive(Serialize, Deserialize)]
    pub struct NewUser {
        name: String,
        age: u8,
        password: String,
        registration_demand_timestamp: u64,
    }
    impl Json for NewUser {
        fn to_bytes_vec(&self) -> Result<Vec<u8>, serde_json::Error> {
            serde_json::to_vec(self)
        }
    }
    impl NewUser {
        pub fn create(name: &str, age: u8, password: &str) -> Self {
            let now = SystemTime::now();
            Self {
                name: name.to_owned(),
                age,
                password: password.to_owned(),
                registration_demand_timestamp: SystemTime::duration_since(&now, UNIX_EPOCH)
                    .expect("time can't be inferior to system_time")
                    .as_secs(),
            }
        }
    }
}
