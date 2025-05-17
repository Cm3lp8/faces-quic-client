pub use queue_builder::{ResponseChannel, ResponseHead, ResponseQueue};
pub use response_builder::PartialResponse;
pub use response_builder::{
    DownloadProgressStatus, Http3Response, ReqStatus, UploadProgressStatus, WaitPeerResponse,
};
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
        sync::Arc,
        usize,
    };

    use log::{debug, error, info, warn};
    use quiche::h3::{self, Header, NameValue};
    use serde::{Deserialize, Serialize};
    use stream_framer::FrameParser;
    use uuid::Uuid;

    use crate::{client_manager::persistant_stream::StreamSub, my_log, RequestEventListener};

    use self::partial_response_impl::{handle_down_stream, respond_once};

    pub struct DownloadProgressStatus {
        req_path: String,
        request_uuid: Uuid,
        progress: f32,
        total: usize,
        received: usize,
    }
    impl Display for DownloadProgressStatus {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "percentage_completed [{}] || written/total [{}/{}]",
                self.progress, self.received, self.total
            )
        }
    }
    impl DownloadProgressStatus {
        pub fn new(
            req_path: &str,
            request_uuid: Uuid,
            received: usize,
            total: usize,
            percentage_completed: f32,
        ) -> Self {
            let value = if percentage_completed > 1.0 {
                1.0
            } else {
                percentage_completed
            };

            Self {
                req_path: req_path.to_string(),
                request_uuid,
                progress: value,
                total,
                received,
            }
        }
        pub fn uuid(&self) -> Uuid {
            self.request_uuid
        }
        pub fn req_path(&self) -> String {
            self.req_path.to_owned()
        }
        pub fn progress(&self) -> f32 {
            self.progress as f32
        }
    }

    #[derive(Clone)]
    pub struct UploadProgressStatus {
        request_uuid: Uuid,
        req_path: String,
        completed: f32,
        total: usize,
        received: usize,
    }
    impl Display for UploadProgressStatus {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "percentage_completed [{}] || written/total [{}/{}]",
                self.completed, self.received, self.completed
            )
        }
    }
    impl UploadProgressStatus {
        pub fn new(
            req_path: &str,
            request_uuid: Uuid,
            received: usize,
            total: usize,
            percentage_completed: f32,
        ) -> Self {
            let value = if percentage_completed > 1.0 {
                1.0
            } else {
                percentage_completed
            };

            Self {
                req_path: req_path.to_string(),
                request_uuid,
                completed: value,
                total,
                received,
            }
        }
        pub fn uuid(&self) -> Uuid {
            self.request_uuid
        }
        pub fn req_path(&self) -> String {
            self.req_path.to_owned()
        }
        pub fn from_bytes(req_path: &str, request_uuid: Uuid, bytes: &[u8]) -> Option<Self> {
            let mut progress: Option<f32> = None;
            let mut written = 0usize;
            let mut total = 0usize;
            for key in String::from_utf8_lossy(bytes).split("%&") {
                if let Some((label, value)) = key.split_once("=") {
                    if label == "progress" {
                        if let Ok(progress_value) = value.parse::<f32>() {
                            progress = Some(progress_value);
                        }
                    }
                    if label == "written" {
                        if let Ok(w) = value.parse::<usize>() {
                            written = w;
                        }
                    }
                    if label == "total" {
                        if let Ok(w) = value.parse::<usize>() {
                            total = w;
                        }
                    }
                }
            }

            if let Some(value) = progress {
                Some(UploadProgressStatus::new(
                    req_path,
                    request_uuid,
                    written,
                    total,
                    value,
                ))
            } else {
                None
            }
        }
        pub fn progress(&self) -> f32 {
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

    pub enum ReqStatus {
        Success {
            stream_id: u64,
            headers: Vec<h3::Header>,
            data: Option<Vec<u8>>,
        },
        Error {
            stream_id: u64,
            headers: Vec<h3::Header>,
            data: Option<Vec<u8>>,
        },
        None,
    }

    impl CompletedResponse {
        pub fn new(stream_id: u64, headers: Vec<h3::Header>, data: Vec<u8>) -> Self {
            Self {
                headers,
                stream_id,
                data,
            }
        }
        pub fn as_data(&self) -> &[u8] {
            &self.data
        }
        pub fn status_code(&self) -> Option<Vec<u8>> {
            if let Some(status) = self.headers.iter().find(|hdr| hdr.name() == b":status") {
                Some(status.value().to_vec())
            } else {
                None
            }
        }
        pub fn raw_data(&mut self) -> Vec<u8> {
            std::mem::replace(&mut self.data, Vec::with_capacity(1))
        }
        pub fn status(&mut self) -> ReqStatus {
            if let Some(status) = self.headers.iter().find(|hdr| hdr.name() == b":status") {
                if status.value() == b"200" || status.value() == b"201" {
                    ReqStatus::Success {
                        stream_id: self.stream_id,
                        headers: std::mem::replace(&mut self.headers, vec![]),
                        data: if self.data.is_empty() {
                            None
                        } else {
                            Some(std::mem::replace(&mut self.data, vec![]))
                        },
                    }
                } else {
                    ReqStatus::Error {
                        stream_id: self.stream_id,
                        headers: std::mem::replace(&mut self.headers, vec![]),
                        data: if self.data.is_empty() {
                            None
                        } else {
                            Some(std::mem::replace(&mut self.data, vec![]))
                        },
                    }
                }
            } else {
                ReqStatus::None
            }
        }
        pub fn has_body(&self) -> bool {
            if self.data.is_empty() {
                false
            } else {
                true
            }
        }
        pub fn is_json(&self) -> bool {
            if let Some(content_type) = self
                .headers()
                .iter()
                .find(|hdr| hdr.name() == b"content-type")
            {
                if content_type.value() == b"application/json" {
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }

        pub fn get_json<'a, T: Deserialize<'a>>(&'a self) -> Result<T, String> {
            if self.is_json() && self.data.len() > 0 {
                return serde_json::from_slice(&self.data).map_err(|e| e.to_string());
            }
            Err("No json".to_string())
        }
        pub fn headers(&self) -> Vec<h3::Header> {
            self.headers.clone()
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
                    write!(
                        f,
                        "body stream_id [{s_i}] : Body packet [{}] bytes",
                        data_len
                    )
                }
                Self::Header(header) => {
                    let s_i = header.stream_id();
                    write!(
                        f,
                        "header stream_id [{s_i}] : header [{:#?}]",
                        header.headers()
                    )
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
        pub fn body_type(&self, req_path: &str, req_uuid: Uuid) -> BodyType {
            BodyType::parse_packet(&self.packet, req_path, req_uuid)
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
        UploadProgressStatusBody(UploadProgressStatus),
        Packet(&'a [u8]),
        Err(()),
    }
    impl<'a> BodyType<'a> {
        pub fn parse_packet(packet: &'a [u8], req_path: &str, req_uuid: Uuid) -> Self {
            if packet.len() < 4 {
                return Self::Packet(packet);
            }

            let (prefix, suffix) = packet.split_at(4);

            if prefix == b"s??%" {
                if let Some(progress_status) =
                    UploadProgressStatus::from_bytes(req_path, req_uuid, suffix)
                {
                    Self::UploadProgressStatusBody(progress_status)
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
        progress_channel: crossbeam::channel::Receiver<UploadProgressStatus>,
    }
    impl WaitPeerResponse {
        pub fn new(
            stream_ids: &(u64, String),
            response_channel: crossbeam::channel::Receiver<CompletedResponse>,
            progress_channel: crossbeam::channel::Receiver<UploadProgressStatus>,
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
        /// With UploadProgressStatus parameter, client can  retrieve some informations on reception status from the server.
        ///
        ///
        ///
        ///
        ///
        ///
        pub fn with_progress_callback(
            self,
            progress_cb: impl Fn(UploadProgressStatus) + Send + 'static,
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
    pub struct PartialResponse {
        stream_id: u64,
        connexion_id: String,
        request_uuid: Uuid,
        req_path: String,
        streamable: Option<StreamSub>,
        event_subscriber: Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
        headers: Option<Vec<h3::Header>>,
        content_length: Option<usize>,
        stream_body_len: Option<usize>,
        stream_data: Vec<u8>,
        data: Vec<u8>,
        packet_count: usize,
        response_channel: (
            crossbeam::channel::Sender<CompletedResponse>,
            crossbeam::channel::Receiver<CompletedResponse>,
        ),
        progress_channel: (
            crossbeam::channel::Sender<UploadProgressStatus>,
            crossbeam::channel::Receiver<UploadProgressStatus>,
        ),
    }
    impl Debug for PartialResponse {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Partial response stream_id [{}] | connexion_id [{}] | request_uuid [{}] | headers [{:?}] | ", self.stream_id, self.connexion_id, self.request_uuid, self.headers)
        }
    }
    impl PartialResponse {
        /// Create a new partial partial response. It is a buffer type that keep track of the
        /// progression of the response.
        /// Create a UUid for this stream for event management purpose in the program that called
        /// this request.
        pub fn new(
            req_path: &str,
            event_subscriber: Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
            stream_ids: &(u64, String),
        ) -> (
            Self,
            crossbeam::channel::Receiver<CompletedResponse>,
            crossbeam::channel::Receiver<UploadProgressStatus>,
        ) {
            let request_uuid = Uuid::new_v4();
            let partial_response = Self {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                request_uuid,
                streamable: None,
                req_path: req_path.to_string(),
                event_subscriber,
                headers: None,
                content_length: None,
                stream_body_len: None,
                packet_count: 0,
                stream_data: vec![],
                data: vec![],
                response_channel: crossbeam::channel::bounded(1),
                progress_channel: crossbeam::channel::bounded(1),
            };
            let response_receiver = partial_response.response_channel.1.clone();
            let progress_receiver = partial_response.progress_channel.1.clone();
            (partial_response, response_receiver, progress_receiver)
        }
        /// Create a new partial partial streamable response. It is a buffer type that keep track of the
        /// progression of the response.
        /// Create a UUid for this stream for event management purpose in the program that called
        /// this request.
        ///
        /// # about streamable
        ///
        /// This response type indicates that program handles continuous connexion interactions.
        pub fn new_streamable(
            req_path: &str,
            event_subscriber: Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
            stream_type: StreamSub,
            stream_ids: &(u64, String),
        ) -> (
            Self,
            crossbeam::channel::Receiver<CompletedResponse>,
            crossbeam::channel::Receiver<UploadProgressStatus>,
        ) {
            let request_uuid = Uuid::new_v4();
            let partial_response = Self {
                stream_id: stream_ids.0,
                connexion_id: stream_ids.1.to_owned(),
                request_uuid,
                streamable: Some(stream_type),
                req_path: req_path.to_string(),
                event_subscriber,
                headers: None,
                content_length: None,
                stream_body_len: None,
                packet_count: 0,
                stream_data: vec![],
                data: vec![],
                response_channel: crossbeam::channel::bounded(1),
                progress_channel: crossbeam::channel::bounded(1),
            };
            let response_receiver = partial_response.response_channel.1.clone();
            let progress_receiver = partial_response.progress_channel.1.clone();
            (partial_response, response_receiver, progress_receiver)
        }

        pub fn has_partial_stream_body(&self) -> bool {
            !self.data.is_empty()
        }
        pub fn update_until_completion(&mut self, data: Vec<u8>) -> Option<Vec<u8>> {
            match self.write_stream_data_buffer(data) {
                Ok(n) => {
                    if n == 0 {
                        return Some(self.get_completed_stream_data());
                    }
                }
                Err(_) => {}
            }
            None
        }
        pub fn start_new_body_reception(&mut self, data: Vec<u8>) -> Option<Vec<u8>> {
            if let Ok((total_len, current_body_chunk)) = data.parse_frame_header() {
                self.set_stream_body_len(total_len);
                match self.write_stream_data_buffer(current_body_chunk) {
                    Ok(n) => {
                        if n == 0 {
                            return Some(self.get_completed_stream_data());
                        }
                    }
                    Err(_) => {}
                }
            }

            None
        }
        pub fn _set_content_length(&mut self, content_length: usize) {
            self.content_length = Some(content_length);
        }
        pub fn set_stream_body_len(&mut self, stream_body_len: usize) {
            self.stream_body_len = Some(stream_body_len);
        }
        pub fn get_completed_stream_data(&mut self) -> Vec<u8> {
            std::mem::replace(&mut self.stream_data, vec![])
        }

        /// write in the stream data buffer, on success, returns the bytes remaining to complete
        /// this body reception.
        pub fn write_stream_data_buffer(&mut self, data: Vec<u8>) -> Result<usize, ()> {
            if let Some(total_body_len) = self.stream_body_len.as_ref() {
                let written = data.len() + self.stream_data.len();
                if written <= *total_body_len {
                    self.stream_data.extend(data);
                    Ok(total_body_len - written)
                } else {
                    Err(())
                }
            } else {
                Err(())
            }
        }
        pub fn has_stream(&self) -> Option<&StreamSub> {
            self.streamable.as_ref()
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn data_len(&self) -> usize {
            self.data.len()
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
                                    self.progress_channel.0.send(UploadProgressStatus::new(
                                        self.req_path.as_str(),
                                        self.request_uuid,
                                        len,
                                        0,
                                        0.0,
                                    ))
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

                    debug!("Headers [{:?}]", headers.headers());
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
                    if let Some(headers) = &self.headers {
                        let status = String::from_utf8_lossy(
                            headers
                                .iter()
                                .find(|hdr| hdr.name() == b":status")
                                .unwrap()
                                .value(),
                        )
                        .parse::<usize>()
                        .unwrap();
                        let content_len = String::from_utf8_lossy(
                            headers
                                .iter()
                                .find(|hdr| hdr.name() == b"content-length")
                                .unwrap()
                                .value(),
                        )
                        .parse::<usize>()
                        .unwrap_or(0);

                        respond_once(self, body, status, content_len, &mut can_delete_in_table);
                        return false;
                    }
                    if let BodyType::UploadProgressStatusBody(progress_status) =
                        body.body_type(self.req_path.as_str(), self.request_uuid)
                    {
                        for sub in &self.event_subscriber {
                            if let Err(e) = sub.on_upload_progress(progress_status.clone()) {
                                error!("Failed to send Upload progress")
                            }
                        }
                        return false;
                    }

                    //in case it is a stream route
                    let mut stream_sub = StreamSub::None;
                    match &self.streamable {
                        Some(strm_sub) => {
                            stream_sub = strm_sub.clone();
                        }
                        None => {}
                    }
                    handle_down_stream(self, body, &stream_sub);

                    //debug!("Error : No headers found for body [{}]", body.stream_id());
                }
            }
            can_delete_in_table
        }
    }

    mod partial_response_impl {
        use std::sync::Arc;

        use log::{debug, error};

        use crate::client_manager::persistant_stream::{StreamControlFlow, StreamEvent, StreamSub};

        use super::{CompletedResponse, Http3ResponseBody, PartialResponse, UploadProgressStatus};

        pub fn handle_down_stream(
            partial_response: &mut PartialResponse,
            body: Http3ResponseBody,
            stream_sub: &StreamSub,
        ) {
            //update partial response with incoming stream body and send to user when all specified
            //bytes length has been read.

            if partial_response.has_partial_stream_body() {
                match partial_response.update_until_completion(body.packet) {
                    Some(completed) => {
                        let stream_event = StreamEvent::new(
                            partial_response.req_path.clone(),
                            partial_response.stream_id,
                            if let Some(hdrs) = partial_response.headers.clone() {
                                hdrs
                            } else {
                                vec![]
                            },
                            completed,
                        );

                        stream_sub.callback(stream_event, StreamControlFlow);
                    }
                    None => {}
                }
            } else {
                match partial_response.start_new_body_reception(body.packet) {
                    Some(completed) => {
                        let stream_event = StreamEvent::new(
                            partial_response.req_path.clone(),
                            partial_response.stream_id,
                            if let Some(hdrs) = partial_response.headers.clone() {
                                hdrs
                            } else {
                                vec![]
                            },
                            completed,
                        );

                        stream_sub.callback(stream_event, StreamControlFlow);
                    }
                    None => {}
                }
            }
        }

        pub fn respond_once(
            partial_response: &mut PartialResponse,
            body: Http3ResponseBody,
            status: usize,
            content_len: usize,
            can_delete_in_table: &mut bool,
        ) {
            let percentage_completed = partial_response.data.len() / content_len;
            if body.packet.len() > 0 {
                partial_response.packet_count += 1;
                partial_response.data.extend_from_slice(body.packet());
            }
            for sub in &partial_response.event_subscriber {
                if let Err(e) = sub.on_download_progress(super::DownloadProgressStatus::new(
                    partial_response.req_path.as_str(),
                    partial_response.request_uuid,
                    partial_response.data.len(),
                    content_len,
                    partial_response.data.len() as f32 / content_len as f32,
                )) {
                    error!("Failed to send Upload progress")
                }
            }

            if body.is_end() {
                if let Some(total_len) = partial_response.content_length {
                    let percentage_completed =
                        partial_response.data.len() as f32 / total_len as f32;

                    for sub in &partial_response.event_subscriber {
                        if let Err(e) = sub.on_upload_progress(UploadProgressStatus::new(
                            partial_response.req_path.as_str(),
                            partial_response.request_uuid,
                            partial_response.data.len(),
                            total_len,
                            percentage_completed,
                        )) {
                            error!("Failed to send Upload progress")
                        }
                    }
                }
                if status != 100 {
                    if let Err(e) =
                        partial_response
                            .response_channel
                            .0
                            .send(CompletedResponse::new(
                                partial_response.stream_id,
                                std::mem::replace(
                                    partial_response.headers.as_mut().unwrap(),
                                    Vec::with_capacity(1),
                                ),
                                std::mem::replace(&mut partial_response.data, vec![]),
                            ))
                    {
                        debug!(
                            "Error: Failed sending complete response for stream_id [{}] -> [{:?}]",
                            body.stream_id(),
                            e
                        );
                    } else {
                        *can_delete_in_table = true;
                    }
                }
            }
        }
    }
}
mod response_manager_worker {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use log::{debug, info, warn};

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
                let table_guard = &mut *partial_table_clone_0.lock().unwrap();
                let (stream_id, conn_id) = server_response.ids();
                let mut delete_entry = false;
                if let Some(entry) = table_guard.get_mut(&(stream_id, conn_id.to_owned())) {
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
