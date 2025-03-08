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

    #[derive(Clone)]
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
        ///
        ///This is the fifo channel that send every response to the response manager unit.
        ///
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
    use std::{
        fmt::{Debug, Display},
        usize,
    };

    use log::{debug, error, info, warn};
    use quiche::h3::{self, Header, NameValue};

    pub struct ProgressStatus {
        completed: usize,
    }
    impl ProgressStatus {
        pub fn new(percentage_completed: usize) -> Self {
            let value = if percentage_completed > 100 {
                100
            } else {
                percentage_completed
            };

            Self { completed: value }
        }
        pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
            let mut progress: Option<usize> = None;
            for key in String::from_utf8_lossy(bytes).split("%&") {
                if let Some((_label, value)) = key.split_once("=") {
                    if let Ok(progress_value) = value.parse::<usize>() {
                        progress = Some(progress_value);
                    }
                }
            }

            if let Some(value) = progress {
                Some(ProgressStatus::new(value))
            } else {
                None
            }
        }
        pub fn progress(&self) -> usize {
            self.completed
        }
    }

    pub struct CompletedResponse {
        headers: Vec<h3::Header>,
        #[allow(warnings)]
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
        ///
        ///
        /// Return true if this server response comes from a stream open by the server
        /// # Example
        ///
        /// let response: Http3Response::new(7, some_id, &[0], false);
        /// let response_1: Http3Response::new(4, some_id, &[0], false);
        ///
        /// assert!(response.is_progress_status_response());
        /// assert!(!response_1.is_progress_status_response());
        ///
        ///
        ///
        pub fn is_progress_status_response(&self) -> bool {
            match self {
                Self::Header(header) => header.stream_id % 4 == 3,
                Self::Body(body) => body.stream_id % 4 == 3,
            }
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
                Http3Response::Header(_headers) => {
                    debug!("Not a body, is an header variant");
                    None
                }

                Http3Response::Body(_) => None,
            }
        }
        pub fn packet(&self) -> Option<&[u8]> {
            match self {
                Http3Response::Header(_) => {
                    debug!("Not a body, is an header variant");
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
        #[allow(warnings)]
        packet_count: usize,
        end: bool,
    }
    impl Http3ResponseBody {
        pub fn new(stream_id: u64, connexion_id: String, packet: Vec<u8>, end: bool) -> Self {
            Self {
                stream_id,
                connexion_id,
                packet,
                packet_count: 0,
                end,
            }
        }
        pub fn body_type(&self) -> BodyType {
            BodyType::parse_packet(&self.packet)
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

    pub enum BodyType<'a> {
        ProgressStatusBody(ProgressStatus),
        Packet(&'a [u8]),
        Err(()),
    }
    impl<'a> BodyType<'a> {
        pub fn parse_packet(packet: &'a [u8]) -> Self {
            if packet.len() < 4 {
                return Self::Packet(packet);
            }

            let (prefix, suffix) = packet.split_at(4);

            if prefix == b"s??%" {
                if let Some(progress_status) = ProgressStatus::from_bytes(suffix) {
                    Self::ProgressStatusBody(progress_status)
                } else {
                    Self::Err(())
                }
            } else {
                Self::Packet(packet)
            }
        }
    }

    #[allow(warnings)]
    pub struct WaitPeerResponse {
        stream_id: u64,
        connexion_id: String,
        response_channel: crossbeam::channel::Receiver<CompletedResponse>,
        progress_channel: crossbeam::channel::Receiver<ProgressStatus>,
    }
    impl WaitPeerResponse {
        pub fn new(
            stream_ids: &(u64, String),
            response_channel: crossbeam::channel::Receiver<CompletedResponse>,
            progress_channel: crossbeam::channel::Receiver<ProgressStatus>,
        ) -> WaitPeerResponse {
            WaitPeerResponse {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                response_channel,
                progress_channel,
            }
        }
        ///
        ///
        /// With ProgressStatus parameter, client can  retrieve some informations on reception status from the server.
        ///
        ///
        ///
        ///
        ///
        ///
        pub fn with_progress_callback(
            self,
            progress_cb: impl Fn(ProgressStatus) + Send + 'static,
        ) -> Self {
            let receiver = self.progress_channel.clone();
            std::thread::Builder::new()
                .stack_size(1024 * 10)
                .spawn(move || {
                    while let Ok(progress_status) = receiver.recv() {
                        progress_cb(progress_status);
                    }
                })
                .unwrap();
            self
        }
        pub fn wait_response(&self) -> Result<CompletedResponse, crossbeam::channel::RecvError> {
            self.response_channel.recv()
        }
    }
    #[derive(Debug)]
    pub struct PartialResponse {
        stream_id: u64,
        connexion_id: String,
        headers: Option<Vec<h3::Header>>,
        content_length: Option<usize>,
        data: Vec<u8>,
        packet_count: usize,
        response_channel: (
            crossbeam::channel::Sender<CompletedResponse>,
            crossbeam::channel::Receiver<CompletedResponse>,
        ),
        progress_channel: (
            crossbeam::channel::Sender<ProgressStatus>,
            crossbeam::channel::Receiver<ProgressStatus>,
        ),
    }
    impl PartialResponse {
        pub fn new(
            stream_ids: &(u64, String),
        ) -> (
            Self,
            crossbeam::channel::Receiver<CompletedResponse>,
            crossbeam::channel::Receiver<ProgressStatus>,
        ) {
            let partial_response = Self {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                headers: None,
                content_length: None,
                packet_count: 0,
                data: vec![],
                response_channel: crossbeam::channel::bounded(1),
                progress_channel: crossbeam::channel::bounded(1),
            };
            let response_receiver = partial_response.response_channel.1.clone();
            let progress_receiver = partial_response.progress_channel.1.clone();
            (partial_response, response_receiver, progress_receiver)
        }
        pub fn _set_content_length(&mut self, content_length: usize) {
            self.content_length = Some(content_length);
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn connexion_id(&self) -> &str {
            self.connexion_id.as_str()
        }
        ///
        /// On Http3Response event, this can trigger the CompletedResponse building and sending to
        /// the client, or stack the bytes if a body is being received.
        ///
        /// return true if data is complete
        ///
        pub fn extend_data(&mut self, server_packet: Http3Response) -> bool {
            let mut can_delete_in_table = false;
            match server_packet {
                Http3Response::Header(headers) => {
                    warn!("Http3 reponse [{:?}]", headers.is_end());
                    if let Some(_status_100) = headers
                        .headers()
                        .iter()
                        .find(|hdr| hdr.name() == b":status" && hdr.value() == b"100")
                    {
                        if let Some(progress) = headers
                            .headers()
                            .iter()
                            .find(|hdr| hdr.name() == b"x-progress")
                        {
                            if let Ok(len) =
                                String::from_utf8_lossy(progress.value()).parse::<usize>()
                            {
                                if let Err(e) =
                                    self.progress_channel.0.send(ProgressStatus::new(len))
                                {
                                    debug!(
                        "Error: Failed sending progress status for stream_id [{}] -> [{:?}]",
                        headers.stream_id(),
                        e
                    );
                                }
                            } else {
                                error!("Failed parsing hdr value : not a digit");
                            }
                        }
                        return false;
                    }
                    self.headers = Some(headers.headers().to_vec());

                    let _content_length = if let Some(content_length) = headers
                        .headers()
                        .iter()
                        .find(|hdr| hdr.name() == b"content-length")
                    {
                        if let Ok(len) =
                            String::from_utf8_lossy(content_length.value()).parse::<usize>()
                        {
                            self.content_length = Some(len);
                        } else {
                            error!("not an digit");
                        };
                    };

                    if headers.is_end() {
                        if let Err(e) = self.response_channel.0.send(CompletedResponse::new(
                            self.stream_id,
                            std::mem::replace(
                                self.headers.as_mut().unwrap(),
                                Vec::with_capacity(1),
                            ),
                            vec![],
                        )) {
                            debug!(
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
                    warn!(
                        "Http3 reponse [{:?}] header [{:?}]",
                        body.is_end(),
                        self.headers.is_some()
                    );
                    if let Some(_headers) = &self.headers {
                        if body.packet.len() > 0 {
                            self.packet_count += 1;
                            self.data.extend_from_slice(body.packet());
                            info!(
                                "body data extended [{:?}] [{:?}]",
                                body.packet.len(),
                                body.is_end()
                            );
                        }

                        if body.is_end() {
                            if let Some(total_len) = self.content_length {
                                let percentage_completed =
                                    ((self.data.len() as f32 / total_len as f32) * 100.0) as usize;

                                if let Err(e) = self
                                    .progress_channel
                                    .0
                                    .send(ProgressStatus::new(percentage_completed))
                                {
                                    error!("Failed sending percentage completion [{:?}]", e);
                                }
                            }
                            if let Err(e) = self.response_channel.0.send(CompletedResponse::new(
                                self.stream_id,
                                std::mem::replace(
                                    self.headers.as_mut().unwrap(),
                                    Vec::with_capacity(1),
                                ),
                                std::mem::replace(&mut self.data, vec![]),
                            )) {
                                debug!(
                        "Error: Failed sending complete response for stream_id [{}] -> [{:?}]",
                        body.stream_id(),
                        e
                    );
                            } else {
                                can_delete_in_table = true;
                            }
                        }
                    } else {
                        if let BodyType::ProgressStatusBody(progress_status) = body.body_type() {
                            if let Err(e) = self.progress_channel.0.send(progress_status) {
                                error!("Failed sending percentage completion [{:?}]", e);
                            }
                            return false;
                        }
                        debug!("Error : No headers found for body [{}]", body.stream_id());
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

    use log::{debug, info};

    use self::{response_builder::PartialResponse, response_mngr::PartialResponseReceiver};

    use super::*;

    ///
    ///Run the partial Response table response manager
    ///
    /// Response queue pops the responseEvent from the server, partial_response_receiver is the
    /// channel that receives the partial responses for registration.
    ///
    pub fn run(response_queue: ResponseQueue, partial_response_receiver: PartialResponseReceiver) {
        let partial_response_table =
            Arc::new(Mutex::new(HashMap::<(u64, String), PartialResponse>::new()));
        let partial_table_clone_0 = partial_response_table.clone();
        let partial_table_clone_1 = partial_response_table.clone();
        std::thread::spawn(move || {
            while let Ok(server_response) = response_queue.pop_response() {
                info!("New packet response : [{:?}]", server_response);
                let table_guard = &mut *partial_table_clone_0.lock().unwrap();
                let (stream_id, conn_id) = server_response.ids();
                let mut delete_entry = false;
                if let Some(entry) = table_guard.get_mut(&(stream_id, conn_id.to_owned())) {
                    info!("extending data...");
                    delete_entry = entry.extend_data(server_response);
                }

                if delete_entry {
                    debug!("Can delete [{stream_id}]");
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
