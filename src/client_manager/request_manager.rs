#![allow(warnings)]
mod request_body;
pub use event_listener::{ProgressTracker, RequestEvent, RequestEventListener};
pub use queue_builder::{RequestChannel, RequestHead, RequestQueue};
pub use request_body::ContentType;
pub use request_body::RequestBody;
pub use request_builder::{
    Http3Request, Http3RequestBuilder, Http3RequestConfirm, Http3RequestPrep, PingStatus,
};
pub use request_format::{BodyType, H3Method};
mod event_listener;
mod queue_builder {
    use std::time::{Duration, Instant};

    use log::{debug, info, warn};
    use quiche::h3::Header;

    use self::{
        request_body::RequestBody,
        request_builder::{BodyRequest, PingStatus},
    };

    use super::*;

    #[derive(Clone)]
    pub struct RequestChannel {
        channel: (
            crossbeam::channel::Sender<(Http3Request, crossbeam::channel::Sender<Instant>)>,
            crossbeam::channel::Receiver<(Http3Request, crossbeam::channel::Sender<Instant>)>,
        ),
    }

    pub struct RequestHead {
        head: crossbeam::channel::Sender<(Http3Request, crossbeam::channel::Sender<Instant>)>,
    }
    impl RequestHead {
        pub fn send_request(
            &self,
            request: (Http3Request, crossbeam::channel::Sender<Instant>),
        ) -> Result<
            (),
            crossbeam::channel::SendError<(Http3Request, crossbeam::channel::Sender<Instant>)>,
        > {
            self.head.send(request)
        }
        ///
        ///Here we split the given body if necessary : Send body in chunks. Open a thread that read the buffer in loop, sending chunk by chunk
        ///the body with the stream_id.
        ///
        ///
        pub fn send_body(&self, stream_id: u64, chunk_size: usize, mut body: RequestBody) {
            let body_sender = self.head.clone();
            std::thread::spawn(move || {
                let mut body = body;
                let body_total_len = body.len();
                let mut byte_send = 0;
                let mut packet_send = 0;

                let number_of_packet = body.len() / chunk_size + {
                    if body.len() % chunk_size == 0 {
                        0
                    } else {
                        1
                    }
                };

                let mut packet_count = 0;
                let send_duration = Instant::now();
                let mut sending_duration = Duration::from_micros(13);
                let mut last_send = Instant::now();
                let mut read_buffer = &mut vec![0; chunk_size].into_boxed_slice();

                while let Ok(n) = body.read(&mut read_buffer) {
                    let now = Instant::now();

                    // std::thread::sleep(sending_duration);
                    let adjust_duration = crossbeam::channel::bounded::<Instant>(1);

                    let end = n + byte_send;
                    let data = read_buffer[..n].to_vec();

                    let body_request = Http3Request::Body(BodyRequest::new(
                        stream_id,
                        packet_count as usize,
                        data,
                        if end == body_total_len { true } else { false },
                    ));

                    if let Err(e) = body_sender.send((body_request, adjust_duration.0)) {
                        debug!("Error : failed sending body packet on stream [{stream_id}] packet send [{packet_send}]");
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(150));
                    byte_send += n;
                    packet_count += 1;
                    last_send = Instant::now();
                    if byte_send == body_total_len {
                        break;
                    }
                }
                debug!(
                    "Body [{}] bytes send succesfully on stream [{stream_id}] in [{}] packets in [{:?}]",
                    byte_send, packet_count, send_duration.elapsed()
                );
            });
        }
        ///
        ///Here we split the given body if necessary : Send body in chunks. Open a thread that read the buffer in loop, sending chunk by chunk
        ///the body with the stream_id.
        ///
        ///# the bodies ending
        ///
        ///The bodies send by this fonction cant terminate a connection since it's a streaming
        ///context. is_end == always false.
        ///
        pub fn send_body_while_streaming(
            &self,
            stream_id: u64,
            chunk_size: usize,
            mut body: RequestBody,
        ) {
            let body_sender = self.head.clone();
            std::thread::spawn(move || {
                let mut body = body;
                let body_total_len = body.len();
                let mut byte_send = 0;
                let mut packet_send = 0;

                let number_of_packet = body.len() / chunk_size + {
                    if body.len() % chunk_size == 0 {
                        0
                    } else {
                        1
                    }
                };

                let mut packet_count = 0;
                let send_duration = Instant::now();
                let mut sending_duration = Duration::from_micros(13);
                let mut last_send = Instant::now();
                let mut read_buffer = &mut vec![0; chunk_size].into_boxed_slice();

                while let Ok(n) = body.read(&mut read_buffer) {
                    let now = Instant::now();

                    // std::thread::sleep(sending_duration);
                    let adjust_duration = crossbeam::channel::bounded::<Instant>(1);

                    let end = n + byte_send;
                    let data = read_buffer[..n].to_vec();

                    let body_request = Http3Request::Body(BodyRequest::new(
                        stream_id,
                        packet_count as usize,
                        data,
                        false, //always false here (streaming context)
                    ));

                    if let Err(e) = body_sender.send((body_request, adjust_duration.0)) {
                        debug!("Error : failed sending body packet on stream [{stream_id}] packet send [{packet_send}]");
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(150));
                    byte_send += n;
                    packet_count += 1;
                    last_send = Instant::now();
                    if byte_send == body_total_len {
                        break;
                    }
                }
                debug!(
                    "Body [{}] bytes send succesfully on stream [{stream_id}] in [{}] packets in [{:?}]",
                    byte_send, packet_count, send_duration.elapsed()
                );
            });
        }
        pub fn send_ping(
            &self,
            ping_status: PingStatus,
        ) -> Result<
            (),
            crossbeam::channel::SendError<(Http3Request, crossbeam::channel::Sender<Instant>)>,
        > {
            let body_sender = self.head.clone();
            let adjust_duration = crossbeam::channel::bounded::<Instant>(1);

            body_sender.send((Http3Request::Ping(ping_status), adjust_duration.0))
        }
    }

    pub struct RequestQueue {
        queue: crossbeam::channel::Receiver<(Http3Request, crossbeam::channel::Sender<Instant>)>,
    }

    impl RequestQueue {
        pub fn pop_request(&self) -> Option<(Http3Request, crossbeam::channel::Sender<Instant>)> {
            if let Ok(new_req) = self.queue.try_recv() {
                debug!("sending next request [{:?}]", new_req);
                Some(new_req)
            } else {
                None
            }
        }
    }
    impl Clone for RequestQueue {
        fn clone(&self) -> Self {
            Self {
                queue: self.queue.clone(),
            }
        }
    }

    impl Clone for RequestHead {
        fn clone(&self) -> Self {
            Self {
                head: self.head.clone(),
            }
        }
    }

    impl RequestChannel {
        pub fn new() -> Self {
            Self {
                channel: crossbeam::channel::unbounded(),
            }
        }
        pub fn get_queue(&self) -> RequestQueue {
            RequestQueue {
                queue: self.channel.1.clone(),
            }
        }
        pub fn get_head(&self) -> RequestHead {
            RequestHead {
                head: self.channel.0.clone(),
            }
        }
    }
}

mod request_builder {
    use core::panic;
    use std::{
        fmt::Debug,
        io::Read,
        net::SocketAddr,
        path::{Path, PathBuf},
        sync::Arc,
        thread::panicking,
        time::Duration,
    };

    use log::{debug, error, info, warn};
    use quiche::h3::{self, Header};
    use ring::error;
    use uuid::Uuid;

    use crate::{client_manager::persistant_stream::KeepAlive, my_log};

    use self::{
        event_listener::RequestEventListener, request_body::RequestBody, request_format::H3Method,
    };

    use super::*;

    ///
    ///Wait for the stream_id given by quic client when sending headers
    ///
    pub struct Http3RequestConfirm {
        response: crossbeam::channel::Receiver<(u64, String)>,
    }
    impl Http3RequestConfirm {
        ///
        ///Block the thread until Client releases back the stream_id for the current_request.
        ///
        ///ids are : stream_id(u64) and connexion id (string)
        ///
        pub fn wait_stream_ids(&self) -> Result<(u64, String), crossbeam::channel::RecvError> {
            self.response.recv()
        }
    }
    ///
    ///Http3RequestPrep prepares the request to be processed by the request manager
    ///
    ///
    ///
    ///
    #[derive(Debug)]
    pub enum Http3RequestPrep {
        Body(Content),
        Header(HeaderRequest),
        Ping(Duration),
        BodyFromFile,
    }
    impl Http3RequestPrep {
        pub fn new(
            peer_socket_address: Option<SocketAddr>,
            req_build_uuid: Uuid,
        ) -> Http3RequestBuilder {
            Http3RequestBuilder {
                method: None,
                path: None,
                scheme: None,
                content_type: None,
                event_subscriber: vec![],
                user_agent: None,
                authority: peer_socket_address,
                custom_headers: None,
                uuid: req_build_uuid,
            }
        }
    }

    type StreamId = u64;

    ///
    ///Http3Request is the headers + backchannel pushed to the request queue, and will be popped by
    ///the quiche client.
    ///Backchannel will send back the given stream_id for this request.
    ///
    ///
    ///
    ///
    pub enum Http3Request {
        Body(BodyRequest),
        Header(HeaderRequest),
        Ping(PingStatus),
        BodyFromFile,
    }

    impl Debug for Http3Request {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Body(body) => {
                    // let data_len = body.data.len();
                    write!(
                        f,
                        "req: body =  stream_id []",
                        //   data_len
                    )
                }
                Self::Header(header) => {
                    write!(f, " req = header [{:#?}]", header.headers())
                }
                Self::BodyFromFile => write!(f, "body from file []"),
                Self::Ping(ping_status) => write!(f, "Ping! "),
            }
        }
    }

    pub struct PingStatus {
        close_ping_emission: bool,
        headers: Vec<h3::Header>,
    }
    impl Default for PingStatus {
        fn default() -> Self {
            Self {
                close_ping_emission: false,
                headers: vec![
                    h3::Header::new(b":method", b"GET"),
                    Header::new(b":path", b"/ping"),
                    Header::new(b":authority", b"chat_client"),
                    Header::new(b":scheme", b":https"),
                ],
            }
        }
    }
    impl PingStatus {
        pub fn close_ping(mut self) -> Self {
            self.close_ping_emission = true;
            self
        }
        pub fn headers(&self) -> &[Header] {
            &self.headers
        }
    }

    #[derive(Debug)]
    pub struct Content {
        payload: RequestBody,
    }
    impl Content {
        pub fn take(self) -> RequestBody {
            self.payload
        }
        pub fn new(request_body: RequestBody) -> Self {
            Self {
                payload: request_body,
            }
        }
    }
    pub struct BodyRequest {
        packet_id: usize,
        stream_id: u64,
        payload: Vec<u8>,
        is_end: bool,
    }
    impl BodyRequest {
        pub fn new(stream_id: u64, packet_id: usize, payload: Vec<u8>, is_end: bool) -> Self {
            BodyRequest {
                packet_id,
                stream_id,
                payload,
                is_end,
            }
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn packet_id(&self) -> usize {
            self.packet_id
        }
        pub fn get_reader<R: Read>(&mut self) -> Result<&mut R, ()> {
            Err(())
        }

        pub fn take_data(&mut self) -> Vec<u8> {
            std::mem::replace(&mut self.payload, Vec::with_capacity(1))
        }
        pub fn data(&self) -> &[u8] {
            &self.payload[..]
        }
        pub fn len(&self) -> usize {
            self.payload.len()
        }

        pub fn is_end(&self) -> bool {
            self.is_end
        }
    }

    #[derive(Debug)]
    pub struct HeaderRequest {
        headers: Vec<h3::Header>,
        stream_id_response: crossbeam::channel::Sender<(u64, String)>,
        is_end: bool,
    }
    impl Clone for HeaderRequest {
        fn clone(&self) -> Self {
            Self {
                headers: self.headers.clone(),
                stream_id_response: self.stream_id_response.clone(),
                is_end: self.is_end,
            }
        }
    }
    impl HeaderRequest {
        /// Header Request: param 0 : true if server can close stream, false if bodies are
        /// following.
        pub fn new(
            is_end: bool,
            stream_id_response: crossbeam::channel::Sender<(u64, String)>,
        ) -> HeaderRequest {
            HeaderRequest {
                headers: vec![],
                stream_id_response,
                is_end,
            }
        }
        pub fn add_header(mut self, name: &str, value: &str) -> Self {
            self.headers
                .push(h3::Header::new(name.as_bytes(), value.as_bytes()));
            self
        }
        pub fn add_header_mut(&mut self, name: &str, value: &str) {
            self.headers
                .push(h3::Header::new(name.as_bytes(), value.as_bytes()));
        }
        pub fn add_header_option(mut self, header_option: Option<h3::Header>) -> Self {
            if let Some(hdr) = header_option {
                self.headers.push(hdr);
            };
            self
        }
        pub fn send_ids(
            &self,
            stream_id: u64,
            connexion_id: &str,
        ) -> Result<(), crossbeam::channel::SendError<(u64, String)>> {
            self.stream_id_response
                .send((stream_id, connexion_id.to_owned()))
        }
        pub fn headers(&self) -> &Vec<Header> {
            &self.headers
        }
        pub fn is_end(&self) -> bool {
            self.is_end
        }
    }

    impl Http3Request {
        pub fn new(peer_socket_address: Option<SocketAddr>, uuid: Uuid) -> Http3RequestBuilder {
            Http3RequestBuilder {
                method: None,
                path: None,
                scheme: None,
                content_type: None,
                event_subscriber: vec![],
                user_agent: None,
                authority: peer_socket_address,
                custom_headers: None,
                uuid,
            }
        }
    }

    /// Request builder : call set_method, set_path, set_user_agent, and build.
    pub struct Http3RequestBuilder {
        method: Option<H3Method>,
        path: Option<String>,
        scheme: Option<String>,
        content_type: Option<String>,
        event_subscriber: Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
        user_agent: Option<String>,
        authority: Option<SocketAddr>,
        custom_headers: Option<Vec<(String, String)>>,
        uuid: Uuid,
    }

    impl Http3RequestBuilder {
        pub fn post_data(&mut self, path: String, data: Vec<u8>) -> &mut Self {
            self.post(path, RequestBody::new_data(data))
        }
        pub fn get_path(&self) -> Option<String> {
            if let Some(path) = self.path.as_ref() {
                Some(path.to_string())
            } else {
                None
            }
        }
        pub fn post_file(&mut self, req_path: String, file_path: impl AsRef<Path>) -> &mut Self {
            self.post(
                req_path,
                RequestBody::new_file_path(file_path.as_ref().to_path_buf()),
            )
        }
        pub fn post_stream(
            &mut self,
            req_path: String,
            stream: Box<dyn Read + Send + 'static>,
        ) -> &mut Self {
            self.post(req_path, RequestBody::new_stream(stream))
        }
        fn post(&mut self, path: String, payload: RequestBody) -> &mut Self {
            self.method = Some(H3Method::POST { payload });
            self.path = Some(path);
            self
        }
        pub fn down_stream(&mut self, path: String, payload: Vec<u8>) -> &mut Self {
            self.method = Some(H3Method::POST {
                payload: RequestBody::new_data(payload),
            });
            self.path = Some(path);
            self
        }
        pub fn get(&mut self, path: String) -> &mut Self {
            self.method = Some(H3Method::GET);
            self.path = Some(path);
            self
        }
        pub fn delete(&mut self, req_path: String, auth_token: String) -> &mut Self {
            self.method = Some(H3Method::DELETE);
            self.path = Some(req_path);
            self.set_header("Authorization".to_string(), auth_token);
            self
        }
        pub fn set_user_agent(&mut self, user_agent: String) -> &mut Self {
            self.user_agent = Some(user_agent);
            self
        }
        pub fn set_header(&mut self, name: String, value: String) -> &mut Self {
            if let None = self.custom_headers {
                self.custom_headers = Some(vec![(name, value)]);
            } else {
                self.custom_headers.as_mut().unwrap().push((name, value));
            }
            self
        }
        pub fn set_content_type(&mut self, content_type: ContentType) -> &mut Self {
            self.content_type = Some(content_type.to_string());
            self
        }
        pub fn subscribe_event(
            &mut self,
            event_listener: Arc<dyn RequestEventListener + 'static + Send + Sync>,
        ) {
            self.event_subscriber.push(event_listener.clone());
        }
        pub fn build_down_stream(
            &mut self,
            keep_alive: &Option<KeepAlive>,
        ) -> Result<
            (
                Vec<Http3RequestPrep>,
                Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
                Option<Http3RequestConfirm>,
            ),
            (),
        > {
            if self.method.is_none() || self.path.is_none() || self.authority.is_none() {
                info!("http3 request, nothing to build !");
                return Err(());
            }

            let event_subscriber = std::mem::replace(&mut self.event_subscriber, vec![]);
            let (sender, receiver) = crossbeam::channel::bounded::<(u64, String)>(1);
            let confirmation = Some(Http3RequestConfirm { response: receiver });

            my_log::debug("building en cours");

            let http_request_prep = match self.method.take().unwrap() {
                H3Method::GET => {
                    let mut hdr_req = HeaderRequest::new(true, sender)
                        .add_header(":method", "GET")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");
                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hdr_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    let mut res = vec![Http3RequestPrep::Header(hdr_req)];
                    res
                }
                H3Method::POST { mut payload } => {
                    /*
                    if payload.len() == 0 {
                        error!("Payload must be > to 0 bytes");
                        return Err(());
                    }*/
                    let mut content_type: Option<h3::Header> = None;
                    if let Some(content_type_set) = &self.content_type {
                        content_type = Some(h3::Header::new(
                            b"content-type",
                            content_type_set.as_bytes(),
                        ));
                    }

                    let mut hdr_req = HeaderRequest::new(false, sender)
                        .add_header(":method", "POST")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header("content-length", payload.len().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        .add_header_option(content_type)
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");
                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hdr_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    let mut res = vec![
                        Http3RequestPrep::Header(hdr_req),
                        //   header request
                        Http3RequestPrep::Body(Content::new(payload)),
                    ];
                    if let Some(ping_frequency) = keep_alive {
                        res.push(Http3RequestPrep::Ping(ping_frequency.duration()))
                    }
                    res
                }
                H3Method::DELETE => {
                    let mut hrd_req = HeaderRequest::new(true, sender)
                        .add_header(":method", "DELETE")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");

                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hrd_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    vec![Http3RequestPrep::Header(hrd_req)]
                }
                _ => vec![],
            };

            if !http_request_prep.is_empty() {
                Ok((http_request_prep, event_subscriber, confirmation))
            } else {
                Err(())
            }
        }
        pub fn build(
            &mut self,
        ) -> Result<
            (
                Vec<Http3RequestPrep>,
                Vec<Arc<dyn RequestEventListener + 'static + Send + Sync>>,
                Option<Http3RequestConfirm>,
            ),
            (),
        > {
            if self.method.is_none() || self.path.is_none() || self.authority.is_none() {
                info!("http3 request, nothing to build !");
                return Err(());
            }

            let event_subscriber = std::mem::replace(&mut self.event_subscriber, vec![]);
            let (sender, receiver) = crossbeam::channel::bounded::<(u64, String)>(1);
            let confirmation = Some(Http3RequestConfirm { response: receiver });

            let http_request_prep = match self.method.take().unwrap() {
                H3Method::GET => {
                    let mut hdr_req = HeaderRequest::new(true, sender)
                        .add_header(":method", "GET")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");
                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hdr_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    vec![Http3RequestPrep::Header(hdr_req)]
                }
                H3Method::POST { mut payload } => {
                    if payload.len() == 0 {
                        error!("Payload must be > to 0 bytes");
                        return Err(());
                    }
                    let mut content_type: Option<h3::Header> = None;
                    if let Some(content_type_set) = &self.content_type {
                        content_type = Some(h3::Header::new(
                            b"content-type",
                            content_type_set.as_bytes(),
                        ));
                    }

                    let mut hdr_req = HeaderRequest::new(false, sender)
                        .add_header(":method", "POST")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header("content-length", payload.len().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        .add_header_option(content_type)
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");
                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hdr_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    vec![
                        Http3RequestPrep::Header(hdr_req),
                        //   header request
                        Http3RequestPrep::Body(Content::new(payload)),
                    ]
                }
                H3Method::DELETE => {
                    let mut hrd_req = HeaderRequest::new(true, sender)
                        .add_header(":method", "DELETE")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        /*
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )*/
                        .add_header("accept", "*/*");

                    if let Some(headers) = &self.custom_headers {
                        for hdr in headers.iter() {
                            hrd_req.add_header_mut(hdr.0.as_str(), hdr.1.as_str());
                        }
                    }
                    vec![Http3RequestPrep::Header(hrd_req)]
                }
                _ => vec![],
            };

            if !http_request_prep.is_empty() {
                Ok((http_request_prep, event_subscriber, confirmation))
            } else {
                Err(())
            }
        }
    }
}

mod request_format {

    use self::request_body::RequestBody;

    use super::*;
    use quiche::h3;
    #[derive(Debug, PartialEq)]
    pub enum H3Method {
        GET,
        POST { payload: RequestBody },
        PUT,
        DELETE,
        STREAM,
    }

    impl H3Method {
        ///
        ///Parse method name from raw bytes.
        ///
        ///
        pub fn parse(input: &[u8]) -> Result<H3Method, ()> {
            match &String::from_utf8_lossy(input)[..] {
                "GET" => Ok(H3Method::GET),
                "POST" => Ok(H3Method::POST {
                    payload: RequestBody::Empty,
                }),
                "PUT" => Ok(H3Method::PUT),
                "DELETE" => Ok(H3Method::DELETE),
                &_ => Err(()),
            }
        }
    }

    #[derive(PartialEq)]
    pub enum BodyType {
        Ping,
        Message(String),
        File(String),
        Array,
    }

    pub struct RequestForm {
        method: H3Method,
        path: &'static str,
        scheme: &'static str,
        authority: Option<&'static str>,
        body_cb: Box<
            dyn Fn(&str, Option<Vec<&str>>) -> Result<(Vec<u8>, Vec<u8>), ()>
                + Send
                + Sync
                + 'static,
        >,
        body_type: BodyType,
    }

    impl PartialEq for RequestForm {
        fn eq(&self, other: &Self) -> bool {
            self.method() == other.method()
                && self.path() == self.path()
                && self.scheme == self.scheme
                && self.authority == other.authority
                && self.body_type() == other.body_type()
        }
    }

    impl RequestForm {
        pub fn new() -> RequestFormBuilder {
            RequestFormBuilder::new()
        }
        pub fn path(&self) -> &'static str {
            self.path
        }
        pub fn method(&self) -> &H3Method {
            &self.method
        }
        pub fn body_type(&self) -> &BodyType {
            &self.body_type
        }

        pub fn build_response(
            &self,
            args: Option<Vec<&str>>,
        ) -> Result<(Vec<h3::Header>, Vec<u8>), ()> {
            if let Ok((body, content_type)) = (self.body_cb)(self.path(), args) {
                let headers = vec![
                    h3::Header::new(b":status", b"200"),
                    h3::Header::new(b"alt-svc", b"h3=\":3000\""),
                    h3::Header::new(b"content-type", &content_type),
                    h3::Header::new(b"content-lenght", body.len().to_string().as_bytes()),
                ];
                return Ok((headers, body));
            }
            //to do
            Err(())
        }
    }

    pub struct RequestFormBuilder {
        method: Option<H3Method>,
        path: Option<&'static str>,
        scheme: Option<&'static str>,
        authority: Option<&'static str>,
        body_cb: Option<
            Box<
                dyn Fn(&str, Option<Vec<&str>>) -> Result<(Vec<u8>, Vec<u8>), ()>
                    /*body, content-type*/
                    + Sync
                    + Send
                    + 'static,
            >,
        >,
        body_type: Option<BodyType>,
    }

    impl RequestFormBuilder {
        pub fn new() -> Self {
            Self {
                method: None,
                path: None,
                scheme: None,
                authority: None,
                body_cb: None,
                body_type: None,
            }
        }

        pub fn build(&mut self) -> RequestForm {
            RequestForm {
                method: self.method.take().unwrap(),
                path: self.path.take().unwrap(),
                scheme: self.scheme.take().expect("expected scheme"),
                authority: self.authority.clone(),
                body_cb: self.body_cb.take().expect("No callback cb set"),
                body_type: self.body_type.take().unwrap(),
            }
        }

        ///
        /// Set the callback for the response. It exposes in parameters 0 = the path of the request,
        /// parameters 1 = the list of args if any "/path?id=foo&name=bar&other=etc"
        ///
        ///The callback returns (body, value of content-type field as bytes)
        ///
        pub fn set_body_callback(
            &mut self,
            body_cb: impl Fn(&str, Option<Vec<&str>>) -> Result<(Vec<u8>, Vec<u8>), ()>
                + Sync
                + Send
                + 'static,
        ) -> &mut Self {
            self.body_cb = Some(Box::new(body_cb));
            self
        }
        ///
        ///Set the http method. H3Method::GET, ::POST, ::PUT, ::DELETE
        ///
        ///
        pub fn set_method(&mut self, method: H3Method) -> &mut Self {
            self.method = Some(method);
            self
        }
        ///
        /// Set the request path as "/home"
        ///
        ///
        pub fn set_path(&mut self, path: &'static str) -> &mut Self {
            self.path = Some(path);
            self
        }
        ///
        /// Set the connexion type : https here
        ///
        pub fn set_scheme(&mut self, scheme: &'static str) -> &mut Self {
            self.scheme = Some(scheme);
            self
        }
        ///
        ///set the request type, in the context of the faces app
        ///
        pub fn set_body_type(&mut self, request_type: BodyType) -> &mut Self {
            self.body_type = Some(request_type);
            self
        }
    }
}

mod test {
    use std::{net::SocketAddr, str::FromStr};

    use quiche::h3::NameValue;
    use test::request_format::BodyType;

    use super::*;

    #[test]
    fn build_post_request() {
        /*
                let mut new_request: Http3RequestBuilder =


                    Http3Request::new(Some(SocketAddr::from_str("127.0.0.1:3000").unwrap()));

                let data = vec![0; 5000];
                let data_len = data.len();
                let body_type = BodyType::Ping;
                let request = new_request
                    .post_data("/post", data)
                    .set_user_agent("Camille")
                    .build();

                assert!(request.is_ok());

                let request = request.unwrap();

                assert!(!request.0.is_empty());
                assert!(request.2.is_some());

                // Get method build only 1 request header
                assert!(request.0.len() == 2);

                let header_r = &request.0[0];
                let body_r = &request.0[1];
                match header_r {
                    Http3RequestPrep::Header(header) => {
                        assert!(true);
                        let hdrs = header.headers();

                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":method" && it.value() == b"POST")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":method" && it.value() == b"GET")
                        {
                            assert!(false)
                        } else {
                            assert!(true)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":authority" && it.value() == b"127.0.0.1:3000")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":scheme" && it.value() == b"https")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs.iter().find(|it| {
                            it.name() == b"content-length" && it.value() == data_len.to_string().as_bytes()
                        }) {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b"user-agent" && it.value() == b"Camille")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                    }
                    Http3RequestPrep::Body(_) => {
                        assert!(false)
                    }
                    Http3RequestPrep::BodyFromFile => {}
                }
        */
    }
    #[test]
    fn build_get_request() {
        /*
                let mut new_request: Http3RequestBuilder =
                    Http3Request::new(Some(SocketAddr::from_str("127.0.0.1:3000").unwrap()));

                let request = new_request.get("/path").set_user_agent("Camille").build();

                assert!(request.is_ok());

                let request = request.unwrap();

                assert!(!request.0.is_empty());
                assert!(request.2.is_some());

                // Get method build only 1 request header
                assert!(request.0.len() == 1);

                let http_request: &Http3RequestPrep = &request.0[0];
                match http_request {
                    Http3RequestPrep::Header(header) => {
                        assert!(true);
                        let hdrs = header.headers();

                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":method" && it.value() == b"GET")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b"method" && it.value() == b"GET")
                        {
                            assert!(false)
                        } else {
                            assert!(true)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":authority" && it.value() == b"127.0.0.1:3000")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":scheme" && it.value() == b"https")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b":scheme" && it.value() == b"https")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                        if let Some(found) = hdrs
                            .iter()
                            .find(|it| it.name() == b"user-agent" && it.value() == b"Camille")
                        {
                            assert!(true)
                        } else {
                            assert!(false)
                        }
                    }
                    Http3RequestPrep::Body(_) => {
                        assert!(false)
                    }
                    Http3RequestPrep::BodyFromFile => {}
                }
        */
    }
}
