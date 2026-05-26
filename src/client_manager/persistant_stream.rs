pub use event_stream_types::{KeepAlive, StreamControlFlow, StreamEvent, StreamSub};

pub use ping_emission::{PingEmissionControl, PingEmitter};
pub use stream_builder::{StreamBuilder, StreamReqId};
mod ping_emission {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use mio::Waker;

    use crate::{
        client_manager::request_manager::{PingStatus, RequestHead},
        my_log,
        thread_controller::{self, ThreadController},
    };

    pub struct PingEmissionControl {
        sender: crossbeam::channel::Sender<PingControlKind>,
    }

    impl PingEmissionControl {
        pub fn stop_keepalive(&self) {
            let _ = self.sender.send(PingControlKind::StopKeepAlive);
        }
    }

    enum PingControlKind {
        StopKeepAlive,
        Continue,
    }

    pub struct PingEmitter;

    impl PingEmitter {
        pub fn run(
            ping_freq: Duration,
            request_sender: &RequestHead,
            stream_id: u64,
            waker: &Arc<Mutex<Option<Waker>>>,
            thread_controller: &ThreadController,
        ) -> PingEmissionControl {
            let (sender, receiver) = crossbeam::channel::bounded(1);

            let request_sender = request_sender.clone();
            let waker = waker.clone();
            let thread_controller = thread_controller.clone();

            std::thread::spawn(move || 'ping: loop {
                if !thread_controller.is_on() {
                    break 'ping;
                };
                // drain control queue if any event
                match receiver.try_recv() {
                    Ok(control) => match control {
                        PingControlKind::StopKeepAlive => {
                            break 'ping;
                        }
                        PingControlKind::Continue => {}
                    },
                    _ => {}
                }
                std::thread::sleep(ping_freq);

                let ping_status = PingStatus::default();

                if let Err(e) = request_sender.send_ping(ping_status) {
                    my_log::debug(e);
                } else {
                    if let Some(waker) = &*waker.lock().unwrap() {
                        let _ = waker.wake();
                    };
                }
            });

            PingEmissionControl { sender }
        }

        pub fn stop_ping(&self) -> Result<(), ()> {
            Err(())
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
        fmt::{write, Debug},
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
        on_failure_cb: Arc<Mutex<Option<Box<dyn FnOnce(usize) + Send + 'static>>>>,
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
                on_failure_cb: Arc::new(Mutex::new(None)),
            }
        }

        /// freq in seconds
        pub fn keep_alive(&mut self, freq_as_sec: u64) -> &mut Self {
            self.keep_alive = Some(KeepAlive::new(freq_as_sec));
            self
        }
        pub fn on_failure_cb(&mut self, cb: impl FnOnce(usize) + Send + 'static) -> &mut Self {
            *self.on_failure_cb.lock().unwrap() = Some(Box::new(cb));
            self
        }

        pub fn open(
            &self,
            cb: impl Fn(StreamEvent, StreamControlFlow) + Send + Sync + 'static,
        ) -> StreamReqId {
            let uuid = self.uuid;

            my_log::log("Stream_opening");

            let stream_req_id = StreamReqId::new(uuid);

            // TODO find a better way to create the stream, that doesn't keep the lock
            if let Some(entry) = self.request_builder.lock().unwrap().get_mut(&uuid) {
                let wait_response = self
                    .request_manager
                    .request_manager_ref()
                    .new_stream_with_builder(entry, &self.keep_alive, cb);

                let failure_cb = self.on_failure_cb.clone();
                let request_manager = self.request_manager.clone();
                std::thread::spawn(move || match wait_response {
                    Ok(response) => match response.wait_response() {
                        Ok(res) => {
                            if let Some(status_code) = res.status_code() {
                                match &status_code[..] {
                                    b"200" => {}
                                    b"401" => execute_failure_cb_if_any(failure_cb, 401),
                                    b"503" => execute_failure_cb_if_any(failure_cb, 503),
                                    b"404" => execute_failure_cb_if_any(failure_cb, 404),
                                    _other => execute_failure_cb_if_any(failure_cb, 0),
                                }
                                my_log::debug(String::from_utf8_lossy(&status_code).to_string());
                            } else {
                                my_log::debug("stream response without status code");
                                execute_failure_cb_if_any(failure_cb, 0);
                            }
                        }
                        Err(e) => {
                            my_log::debug(format!("stream wait_response failure [{:?}]", e));
                            if request_manager.is_off() {
                                execute_failure_cb_if_any(failure_cb, 0);
                            } else {
                                my_log::debug("stream wait_response timeout ignored while client is still connected");
                            }
                        }
                    },
                    Err(e) => {
                        my_log::debug(format!("stream opening failure [{:?}]", e));
                        execute_failure_cb_if_any(failure_cb, 0);
                    }
                });
            };
            stream_req_id
        }
    }

    fn execute_failure_cb_if_any(
        cb: Arc<Mutex<Option<Box<dyn FnOnce(usize) + Send + 'static>>>>,
        error_code: usize,
    ) {
        let mut cb_opt = {
            let guard = &mut *cb.lock().unwrap();
            guard.take()
        };

        match cb_opt {
            Some(cb) => cb(error_code),
            _ => {}
        }
    }
    #[derive(Clone)]
    pub struct StreamReqId {
        uuid: Uuid,
    }

    impl StreamReqId {
        pub fn new(uuid: Uuid) -> Self {
            Self { uuid }
        }
        pub fn get_id(&self) -> Uuid {
            self.uuid
        }
    }
}
