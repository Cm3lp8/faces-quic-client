pub use event_stream_types::{KeepAlive, StreamControlFlow, StreamEvent, StreamSub};
pub use ping_emission::PingEmitter;
pub use stream_builder::StreamBuilder;
mod ping_emission {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use mio::Waker;

    use crate::{
        client_manager::request_manager::{PingStatus, RequestHead},
        my_log,
    };

    pub struct PingEmissionControl {
        sender: crossbeam::channel::Sender<()>,
    }

    pub struct PingEmitter;

    impl PingEmitter {
        pub fn run(
            ping_freq: Duration,
            request_sender: &RequestHead,
            stream_id: u64,
            waker: &Arc<Mutex<Option<Waker>>>,
        ) -> PingEmissionControl {
            let (sender, receiver) = crossbeam::channel::bounded(1);

            let request_sender = request_sender.clone();
            let waker = waker.clone();

            std::thread::spawn(move || {
                while let Err(_) = receiver.try_recv() {
                    std::thread::sleep(ping_freq);

                    let ping_status = PingStatus::default();

                    if let Err(e) = request_sender.send_ping(ping_status) {
                        my_log::debug(e);
                    } else {
                        if let Some(waker) = &*waker.lock().unwrap() {
                            let _ = waker.wake();
                        };
                    }
                }
            });

            PingEmissionControl { sender }
        }
    }
}

mod event_stream_types {
    use std::{sync::Arc, time::Duration};

    use quiche::h3;

    #[derive(Clone)]
    pub enum StreamSub {
        UpStream(Arc<dyn Fn(StreamEvent, StreamControlFlow) + Send + Sync + 'static>),
        Downstream(Arc<dyn Fn(StreamEvent, StreamControlFlow) + Send + Sync + 'static>),
        None, //Bidi,
    }
    impl StreamSub {
        pub fn callback(&self, stream_event: StreamEvent, control_flow: StreamControlFlow) {
            match self {
                Self::Downstream(cb) => (*cb)(stream_event, control_flow),
                _ => {}
            }
        }
    }

    pub struct StreamEvent {
        req_path: String,
        stream_id: u64,
        headers: Vec<h3::Header>,
        body: Vec<u8>,
    }

    impl StreamEvent {
        pub fn new(
            req_path: String,
            stream_id: u64,
            headers: Vec<h3::Header>,
            body: Vec<u8>,
        ) -> Self {
            Self {
                req_path,
                stream_id,
                headers,
                body,
            }
        }
        pub fn body_as_slice(&self) -> &[u8] {
            &self.body
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
    }
    pub struct StreamControlFlow;

    pub struct KeepAlive {
        duration: Duration,
    }
    impl KeepAlive {
        pub fn new(duration_as_sec: u64) -> Self {
            Self {
                duration: Duration::from_secs(duration_as_sec),
            }
        }
        pub fn duration(&self) -> Duration {
            self.duration
        }
    }
}

mod stream_builder {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use uuid::Uuid;

    use crate::{
        client_manager::request_manager::{self, Http3RequestBuilder},
        my_log, Http3ClientManager,
    };

    use super::{event_stream_types::KeepAlive, StreamControlFlow, StreamEvent};

    pub struct StreamBuilder {
        uuid: Uuid,
        request_builder: Arc<Mutex<HashMap<Uuid, Http3RequestBuilder>>>,
        request_manager: Http3ClientManager,
        keep_alive: Option<KeepAlive>,
    }

    impl StreamBuilder {
        pub fn with_request_map(
            uuid: Uuid,
            request_builder: Arc<Mutex<HashMap<Uuid, Http3RequestBuilder>>>,
            request_manager: &Http3ClientManager,
        ) -> Self {
            Self {
                uuid,
                request_builder,
                request_manager: request_manager.clone(),
                keep_alive: None,
            }
        }

        pub fn keep_alive(&mut self, freq_as_sec: u64) -> &mut Self {
            self.keep_alive = Some(KeepAlive::new(freq_as_sec));
            self
        }

        pub fn open(&self, cb: impl Fn(StreamEvent, StreamControlFlow) + Send + Sync + 'static) {
            let uuid = self.uuid;

            my_log::log("Stream_opening");

            if let Some(entry) = self.request_builder.lock().unwrap().get_mut(&uuid) {
                let wait_response = self
                    .request_manager
                    .request_manager_ref()
                    .new_stream_with_builder(entry, &self.keep_alive, cb);

                std::thread::spawn(move || {
                    if let Ok(response) = wait_response {
                        if let Ok(res) = response.wait_response() {
                            my_log::debug(String::from_utf8(res.status_code().unwrap().to_vec()));
                        }
                    }
                });
            };
        }
    }
}
