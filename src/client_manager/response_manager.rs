pub use queue_builder::{ResponseChannel, ResponseHead, ResponseQueue};
pub use response_builder::PartialResponse;
pub use response_builder::{Http3Response, WaitPeerResponse};
pub use response_mngr::ResponseManager;

mod response_mngr {
    use std::sync::{Arc, Mutex};

    use super::*;

    pub struct ResponseManager {
        response_queue: ResponseQueue,
        partial_response_channel: (
            crossbeam::channel::Sender<PartialResponse>,
            crossbeam::channel::Receiver<PartialResponse>,
        ),
        is_running: Arc<Mutex<bool>>,
    }
    impl ResponseManager {
        pub fn new(response_queue: ResponseQueue) -> Self {
            Self {
                response_queue,
                partial_response_channel: crossbeam::channel::unbounded(),
                is_running: Arc::new(Mutex::new(false)),
            }
        }
        pub fn run(&self) {
            let guard = &mut *self.is_running.lock().unwrap();
            if !*guard {
                response_manager_worker::run(
                    self.response_queue.clone(),
                    PartialResponseReceiver::new(self.partial_response_channel.1.clone()),
                );
                *guard = true;
            }
        }
        pub fn submitter(&self) -> PartialResponseSubmitter {
            PartialResponseSubmitter {
                sender: self.partial_response_channel.0.clone(),
            }
        }
    }
    impl Clone for ResponseManager {
        fn clone(&self) -> Self {
            Self {
                response_queue: self.response_queue.clone(),
                is_running: self.is_running.clone(),
                partial_response_channel: self.partial_response_channel.clone(),
            }
        }
    }
    pub struct PartialResponseReceiver {
        receiver: crossbeam::channel::Receiver<PartialResponse>,
    }
    impl PartialResponseReceiver {
        pub fn recv(&self) -> Result<PartialResponse, crossbeam::channel::RecvError> {
            self.receiver.recv()
        }
        pub fn new(
            receiver: crossbeam::channel::Receiver<PartialResponse>,
        ) -> PartialResponseReceiver {
            PartialResponseReceiver { receiver }
        }
    }
    pub struct PartialResponseSubmitter {
        sender: crossbeam::channel::Sender<PartialResponse>,
    }
    impl PartialResponseSubmitter {
        ///
        ///Submit the Empty PartialResponse with the CompletedResponse queue receiver
        ///
        ///
        pub fn submit(
            &self,
            partial_response: PartialResponse,
        ) -> Result<(), crossbeam::channel::SendError<PartialResponse>> {
            self.sender.send(partial_response)
        }
    }
}
mod queue_builder {
    use super::*;

    pub struct ResponseChannel {
        channel: (
            crossbeam::channel::Sender<Http3Response>,
            crossbeam::channel::Receiver<Http3Response>,
        ),
    }
    pub struct ResponseHead {
        head: crossbeam::channel::Sender<Http3Response>,
    }

    ///
    ///The end of the queue that pull Response from the client
    ///
    ///
    pub struct ResponseQueue {
        queue: crossbeam::channel::Receiver<Http3Response>,
    }
    impl ResponseQueue {
        pub fn pop_response(&self) -> Result<Http3Response, crossbeam::channel::RecvError> {
            self.queue.recv()
        }
    }

    impl Clone for ResponseQueue {
        fn clone(&self) -> Self {
            Self {
                queue: self.queue.clone(),
            }
        }
    }

    impl ResponseChannel {
        pub fn new() -> Self {
            Self {
                channel: crossbeam::channel::unbounded(),
            }
        }
        pub fn get_head(&self) -> ResponseHead {
            ResponseHead {
                head: self.channel.0.clone(),
            }
        }
        pub fn get_queue(&self) -> ResponseQueue {
            ResponseQueue {
                queue: self.channel.1.clone(),
            }
        }
    }
}

mod response_builder {
    use super::*;

    pub struct CompletedResponse {
        stream_id: u64,
        data: Vec<u8>,
    }

    ///
    ///
    ///A partial response packet from the server.
    ///
    ///
    pub struct Http3Response {
        stream_id: u64,
        packet: Vec<u8>,
        end: bool,
    }
    impl Http3Response {
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn packet(&self) -> &[u8] {
            &self.packet[..]
        }
        pub fn len(&self) -> usize {
            self.packet.len()
        }
    }

    pub struct WaitPeerResponse {
        stream_id: u64,
        receiver: crossbeam::channel::Receiver<CompletedResponse>,
    }
    impl WaitPeerResponse {
        pub fn new(
            stream_id: u64,
            receiver: crossbeam::channel::Receiver<CompletedResponse>,
        ) -> WaitPeerResponse {
            WaitPeerResponse {
                stream_id,
                receiver,
            }
        }
    }
    pub struct PartialResponse {
        stream_id: u64,
        data: Vec<u8>,
        channel: (
            crossbeam::channel::Sender<CompletedResponse>,
            crossbeam::channel::Receiver<CompletedResponse>,
        ),
    }
    impl PartialResponse {
        pub fn new(stream_id: u64) -> (Self, crossbeam::channel::Receiver<CompletedResponse>) {
            let partial_response = Self {
                stream_id,
                data: vec![],
                channel: crossbeam::channel::bounded(1),
            };
            let receiver = partial_response.channel.1.clone();
            (partial_response, receiver)
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
    }
}
mod response_manager_worker {
    use std::{
        collections::HashMap,
        hash::Hash,
        sync::{Arc, Mutex},
    };

    use self::{
        response_builder::PartialResponse,
        response_mngr::{PartialResponseReceiver, PartialResponseSubmitter},
    };

    use super::*;

    pub fn run(response_queue: ResponseQueue, partial_response_receiver: PartialResponseReceiver) {
        let partial_response_table = Arc::new(Mutex::new(HashMap::<u64, PartialResponse>::new()));
        let partial_table_clone_0 = partial_response_table.clone();
        let partial_table_clone_1 = partial_response_table.clone();
        std::thread::spawn(
            move || {
                while let Ok(server_response) = response_queue.pop_response() {}
            },
        );

        std::thread::spawn(move || {
            while let Ok(partial_response_submission) = partial_response_receiver.recv() {
                let stream_id = partial_response_submission.stream_id();
                let table_guard = &mut *partial_table_clone_0.lock().unwrap();

                table_guard.insert(stream_id, partial_response_submission);
            }
        });
    }
}
