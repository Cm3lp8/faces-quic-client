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
        ///
        ///Process incoming response from the peer
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
        // Get the handle to submit PartialResponse to the response manager.
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
        ///Submit the Empty PartialResponse.
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
    impl ResponseHead {
        pub fn send_response(
            &self,
            response: Http3Response,
        ) -> Result<(), crossbeam::channel::SendError<Http3Response>> {
            self.head.send(response)
        }
    }
    impl Clone for ResponseHead {
        fn clone(&self) -> Self {
            Self {
                head: self.head.clone(),
            }
        }
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
    use std::fmt::{Debug, Display};

    use quiche::h3::{self, Header};

    use super::*;

    pub struct CompletedResponse {
        headers: Vec<h3::Header>,
        stream_id: u64,
        data: Vec<u8>,
    }
    impl Display for CompletedResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "[{:#?}]\n data length : {} bytes\n",
                self.headers,
                self.data.len()
            )
        }
    }

    impl CompletedResponse {
        pub fn new(stream_id: u64, headers: Vec<h3::Header>, data: Vec<u8>) -> Self {
            Self {
                headers,
                stream_id,
                data,
            }
        }
        pub fn take_data(&mut self) -> Vec<u8> {
            std::mem::replace(&mut self.data, Vec::with_capacity(1))
        }
    }
    ///
    ///
    ///A partial response packet from the server.
    ///
    ///
    pub enum Http3Response {
        Header(Http3ResponseHeader),
        Body(Http3ResponseBody),
    }
    impl Debug for Http3Response {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Body(body) => {
                    let s_i = body.stream_id();
                    let data_len = body.packet.len();
                    write!(f, "stream_id [{s_i}] : Body packet [{}] bytes", data_len)
                }
                Self::Header(header) => {
                    let s_i = header.stream_id();
                    write!(f, "stream_id [{s_i}] : header [{:#?}]", header.headers())
                }
            }
        }
    }
    impl Http3Response {
        pub fn new_body_data(
            stream_id: u64,
            connexion_id: String,
            packet: &[u8],
            end: bool,
        ) -> Self {
            Self::Body(Http3ResponseBody::new(
                stream_id,
                connexion_id,
                packet.to_vec(),
                end,
            ))
        }
        /// Create new header response type.
        /// Boolean indique if end of stream.
        pub fn new_header(
            stream_id: u64,
            connexion_id: String,
            headers: Vec<h3::Header>,
            end: bool,
        ) -> Self {
            Self::Header(Http3ResponseHeader::new(
                stream_id,
                connexion_id,
                headers,
                end,
            ))
        }
        pub fn ids(&self) -> (u64, String) {
            match self {
                Http3Response::Header(headers) => {
                    (headers.stream_id, headers.connexion_id.to_owned())
                }
                Http3Response::Body(body) => (body.stream_id, body.connexion_id.to_owned()),
            }
        }
        pub fn stream_id(&self) -> u64 {
            match self {
                Http3Response::Header(headers) => headers.stream_id,
                Http3Response::Body(body) => body.stream_id,
            }
        }
        pub fn connexion_id(&self) -> &String {
            match self {
                Http3Response::Header(headers) => &headers.connexion_id,
                Http3Response::Body(body) => &body.connexion_id,
            }
        }
        pub fn headers(&self) -> Option<Vec<h3::Header>> {
            match self {
                Http3Response::Header(headers) => {
                    println!("Not a body, is an header variant");
                    None
                }

                Http3Response::Body(_) => None,
            }
        }
        pub fn packet(&self) -> Option<&[u8]> {
            match self {
                Http3Response::Header(_) => {
                    println!("Not a body, is an header variant");
                    None
                }

                Http3Response::Body(body) => Some(&body.packet[..]),
            }
        }
        pub fn len(&self) -> Option<usize> {
            match self {
                Http3Response::Header(_) => None,
                Http3Response::Body(body) => Some(body.packet.len()),
            }
        }
        pub fn is_end(&self) -> bool {
            match self {
                Http3Response::Header(headers) => headers.end,
                Http3Response::Body(body) => body.end,
            }
        }
    }
    pub struct Http3ResponseHeader {
        stream_id: u64,
        connexion_id: String,
        headers: Vec<h3::Header>,
        end: bool,
    }
    impl Http3ResponseHeader {
        pub fn new(
            stream_id: u64,
            connexion_id: String,
            headers: Vec<h3::Header>,
            end: bool,
        ) -> Self {
            Self {
                stream_id,
                connexion_id,
                headers,
                end,
            }
        }
        pub fn ids(&self) -> (u64, String) {
            (self.stream_id, self.connexion_id.to_owned())
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn connexion_id(&self) -> &String {
            &self.connexion_id
        }
        pub fn headers(&self) -> &Vec<Header> {
            &self.headers
        }
        pub fn is_end(&self) -> bool {
            self.end
        }
    }
    pub struct Http3ResponseBody {
        stream_id: u64,
        connexion_id: String,
        packet: Vec<u8>,
        end: bool,
    }
    impl Http3ResponseBody {
        pub fn new(stream_id: u64, connexion_id: String, packet: Vec<u8>, end: bool) -> Self {
            Self {
                stream_id,
                connexion_id,
                packet,
                end,
            }
        }
        pub fn ids(&self) -> (u64, String) {
            (self.stream_id, self.connexion_id.to_owned())
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn connexion_id(&self) -> &String {
            &self.connexion_id
        }
        pub fn packet(&self) -> &[u8] {
            &self.packet[..]
        }
        pub fn len(&self) -> usize {
            self.packet.len()
        }
        pub fn is_end(&self) -> bool {
            self.end
        }
    }

    pub struct WaitPeerResponse {
        stream_id: u64,
        connexion_id: String,
        receiver: crossbeam::channel::Receiver<CompletedResponse>,
    }
    impl WaitPeerResponse {
        pub fn new(
            stream_ids: &(u64, String),
            receiver: crossbeam::channel::Receiver<CompletedResponse>,
        ) -> WaitPeerResponse {
            WaitPeerResponse {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                receiver,
            }
        }
        pub fn wait_response(&self) -> Result<CompletedResponse, crossbeam::channel::RecvError> {
            self.receiver.recv()
        }
    }
    #[derive(Debug)]
    pub struct PartialResponse {
        stream_id: u64,
        connexion_id: String,
        headers: Option<Vec<h3::Header>>,
        data: Vec<u8>,
        channel: (
            crossbeam::channel::Sender<CompletedResponse>,
            crossbeam::channel::Receiver<CompletedResponse>,
        ),
    }
    impl PartialResponse {
        pub fn new(
            stream_ids: &(u64, String),
        ) -> (Self, crossbeam::channel::Receiver<CompletedResponse>) {
            let partial_response = Self {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                headers: None,
                data: vec![],
                channel: crossbeam::channel::bounded(1),
            };
            let receiver = partial_response.channel.1.clone();
            (partial_response, receiver)
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn connexion_id(&self) -> &str {
            self.connexion_id.as_str()
        }
        pub fn extend_data(&mut self, server_packet: Http3Response) -> bool {
            let mut can_delete_in_table = false;
            match server_packet {
                Http3Response::Header(headers) => {
                    self.headers = Some(headers.headers().to_vec());
                    if headers.is_end() {
                        if let Err(e) = self.channel.0.send(CompletedResponse::new(
                            self.stream_id,
                            std::mem::replace(
                                self.headers.as_mut().unwrap(),
                                Vec::with_capacity(1),
                            ),
                            vec![],
                        )) {
                            println!(
                        "Error: Failed sending complete response for stream_id [{}] -> [{:?}]",
                        headers.stream_id(),
                        e
                    );
                        } else {
                            can_delete_in_table = true;
                        }
                    }
                }
                Http3Response::Body(body) => {
                    println!("is body");
                    if let Some(_headers) = &self.headers {
                        if body.packet.len() > 0 {
                            self.data.extend_from_slice(body.packet());
                            println!("body data extended [{:?}]", body.packet.len());
                        }

                        if body.is_end() {
                            if let Err(e) = self.channel.0.send(CompletedResponse::new(
                                self.stream_id,
                                std::mem::replace(
                                    self.headers.as_mut().unwrap(),
                                    Vec::with_capacity(1),
                                ),
                                std::mem::replace(&mut self.data, vec![]),
                            )) {
                                println!(
                        "Error: Failed sending complete response for stream_id [{}] -> [{:?}]",
                        body.stream_id(),
                        e
                    );
                            } else {
                                can_delete_in_table = true;
                            }
                        }
                    } else {
                        println!("Error : No headers found for body [{}]", body.stream_id());
                    }
                }
            }
            can_delete_in_table
        }
    }
}
mod response_manager_worker {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use self::{
        response_builder::PartialResponse,
        response_mngr::{PartialResponseReceiver, PartialResponseSubmitter},
    };

    use super::*;

    ///
    ///Run the partial Response table response manager
    ///
    ///
    ///
    pub fn run(response_queue: ResponseQueue, partial_response_receiver: PartialResponseReceiver) {
        let partial_response_table =
            Arc::new(Mutex::new(HashMap::<(u64, String), PartialResponse>::new()));
        let partial_table_clone_0 = partial_response_table.clone();
        let partial_table_clone_1 = partial_response_table.clone();
        std::thread::spawn(move || {
            while let Ok(server_response) = response_queue.pop_response() {
                println!("New packet response : [{:?}]", server_response);
                let table_guard = &mut *partial_table_clone_0.lock().unwrap();
                let (stream_id, conn_id) = server_response.ids();
                let mut delete_entry = false;
                if let Some(entry) = table_guard.get_mut(&(stream_id, conn_id.to_owned())) {
                    println!("extending data...");
                    delete_entry = entry.extend_data(server_response);
                } else {
                    for table_g in table_guard.iter() {
                        println!("table [{:#?}]", table_g);
                    }
                }
                if delete_entry {
                    println!("Can delete [{stream_id}]");
                    table_guard.remove(&(stream_id, conn_id.to_owned()));
                }
            }
        });

        std::thread::spawn(move || {
            while let Ok(partial_response_submission) = partial_response_receiver.recv() {
                let stream_id = partial_response_submission.stream_id();
                let conn_id = partial_response_submission.connexion_id();
                let table_guard = &mut *partial_table_clone_1.lock().unwrap();

                table_guard.insert((stream_id, conn_id.to_owned()), partial_response_submission);
            }
        });
    }
}
