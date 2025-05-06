pub use event_stream_types::{KeepAlive, StreamControlFlow, StreamEvent};
pub use ping_emission::PingEmitter;
pub use stream_builder::StreamBuilder;
mod ping_emission {
    use std::time::Duration;

    use crate::client_manager::request_manager::RequestHead;

    pub struct PingEmissionControl {
        sender: crossbeam::channel::Sender<()>,
    }

    pub struct PingEmitter;

    impl PingEmitter {
        pub fn run(
            ping_freq: Duration,
            request_sender: &RequestHead,
            stream_id: u64,
        ) -> PingEmissionControl {
            let (sender, receiver) = crossbeam::channel::bounded(1);

            let request_sender = request_sender.clone();

            std::thread::spawn(move || {
                while let Err(_) = receiver.try_recv() {
                    std::thread::sleep(ping_freq);

                    request_sender.send_ping(stream_id);
                }
            });

            PingEmissionControl { sender }
        }
    }
}

mod event_stream_types {
    use std::time::Duration;

    pub struct StreamEvent;
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
        Http3ClientManager,
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

        pub fn open(&self, cb: impl Fn(StreamEvent, StreamControlFlow)) {
            let uuid = self.uuid;

            if let Some(entry) = self.request_builder.lock().unwrap().get_mut(&uuid) {
                self.request_manager
                    .request_manager_ref()
                    .new_stream_with_builder(entry, &self.keep_alive, cb);
            };
        }
    }
}
