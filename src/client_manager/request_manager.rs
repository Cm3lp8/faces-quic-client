#![allow(warnings)]
pub use queue_builder::{RequestChannel, RequestHead, RequestQueue};
pub use request_builder::{Http3Request, Http3RequestBuilder, Http3RequestConfirm};
pub use request_format::{BodyType, H3Method};
mod queue_builder {
    use std::time::{Duration, Instant};

    use log::{debug, info};
    use quiche::h3::Header;

    use self::request_builder::BodyRequest;

    use super::*;

    #[derive(Clone)]
    pub struct RequestChannel {
        channel: (
            crossbeam::channel::Sender<(Http3Request, crossbeam::channel::Sender<Duration>)>,
            crossbeam::channel::Receiver<(Http3Request, crossbeam::channel::Sender<Duration>)>,
        ),
    }

    pub struct RequestHead {
        head: crossbeam::channel::Sender<(Http3Request, crossbeam::channel::Sender<Duration>)>,
    }
    impl RequestHead {
        pub fn send_request(
            &self,
            request: (Http3Request, crossbeam::channel::Sender<Duration>),
        ) -> Result<
            (),
            crossbeam::channel::SendError<(Http3Request, crossbeam::channel::Sender<Duration>)>,
        > {
            self.head.send(request)
        }
        ///
        ///Here we split the given body if necessary : Send body in chunks. Open a thread that read the buffer in loop, sending chunk by chunk
        ///the body with the stream_id.
        ///
        ///
        pub fn send_body(&self, stream_id: u64, chunk_size: usize, body: Vec<u8>) {
            let body_sender = self.head.clone();
            std::thread::spawn(move || {
                let body = body;
                let mut byte_send = 0;
                let mut packet_send = 0;

                let number_of_packet = body.len() / chunk_size + {
                    if body.len() % chunk_size == 0 {
                        0
                    } else {
                        1
                    }
                };
                log::info!("Sending Body in [{}] packets", number_of_packet);

                let mut packet_count = 0;
                let send_duration = Instant::now();
                let mut sending_duration = Duration::from_micros(12);
                while byte_send < body.len() {
                    std::thread::sleep(sending_duration);
                    let adjust_duration = crossbeam::channel::bounded::<Duration>(1);
                    let end = if byte_send + chunk_size <= body.len() {
                        let end = chunk_size + byte_send;
                        debug!("bytes_send [{:?}] end[{:?}]", byte_send, end);
                        let data = body[byte_send..end].to_vec();

                        let body_request = Http3Request::Body(BodyRequest::new(
                            stream_id,
                            packet_count as usize,
                            data,
                            if end >= body.len() { true } else { false },
                        ));

                        if let Err(e) = body_sender.send((body_request, adjust_duration.0)) {
                            debug!("Error : failed sending body packet on stream [{stream_id}] packet send [{packet_send}]");
                            break;
                        }
                        byte_send += chunk_size;
                        end
                    } else {
                        let end = byte_send + (body.len() - byte_send);
                        debug!("bytes_send [{:?}] end[{:?}]", byte_send, end);
                        let data = body[byte_send..end].to_vec();

                        let body_request = Http3Request::Body(BodyRequest::new(
                            stream_id,
                            packet_count as usize,
                            data,
                            if end >= body.len() { true } else { false },
                        ));

                        if let Err(e) = body_sender.send((body_request, adjust_duration.0)) {
                            debug!("Error : failed sending body packet on stream [{stream_id}] packet send [{packet_send}]");
                            break;
                        }
                        byte_send += body.len() - byte_send;
                        end
                    };

                    packet_count += 1;
                    if let Ok(new_duration) = adjust_duration.1.recv() {
                        sending_duration = new_duration;
                    }
                }
                info!(
                    "Body [{}] bytes send succesfully on stream [{stream_id}] in [{}] packets in [{:?}]",
                    byte_send, packet_count, send_duration.elapsed()
                );
            });
        }
    }

    pub struct RequestQueue {
        queue: crossbeam::channel::Receiver<(Http3Request, crossbeam::channel::Sender<Duration>)>,
    }

    impl RequestQueue {
        pub fn pop_request(&self) -> Option<(Http3Request, crossbeam::channel::Sender<Duration>)> {
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
    use std::{fmt::Debug, net::SocketAddr};

    use log::debug;
    use quiche::h3::{self, Header};

    use self::request_format::H3Method;

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
        BodyFromFile,
    }

    impl Debug for Http3Request {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Body(body) => {
                    let s_i = body.stream_id();
                    let data_len = body.data.len();
                    write!(
                        f,
                        "req: body =  stream_id [{s_i}] : Body packet [{}] bytes",
                        data_len
                    )
                }
                Self::Header(header) => {
                    write!(f, " req = header [{:#?}]", header.headers())
                }
                Self::BodyFromFile => write!(f, "body from file []"),
            }
        }
    }
    pub struct BodyRequest {
        packet_id: usize,
        stream_id: u64,
        data: Vec<u8>,
        is_end: bool,
    }
    impl BodyRequest {
        pub fn new(stream_id: u64, packet_id: usize, data: Vec<u8>, is_end: bool) -> Self {
            BodyRequest {
                packet_id,
                stream_id,
                data,
                is_end,
            }
        }
        pub fn stream_id(&self) -> u64 {
            self.stream_id
        }
        pub fn packet_id(&self) -> usize {
            self.packet_id
        }
        pub fn take_data(&mut self) -> Vec<u8> {
            std::mem::replace(&mut self.data, Vec::with_capacity(1))
        }
        pub fn data(&self) -> &[u8] {
            &self.data[..]
        }
        pub fn len(&self) -> usize {
            self.data.len()
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
        pub fn new(peer_socket_address: Option<SocketAddr>) -> Http3RequestBuilder {
            Http3RequestBuilder {
                method: None,
                path: None,
                scheme: None,
                user_agent: None,
                authority: peer_socket_address,
                custom_headers: None,
            }
        }
    }

    /// Request builder : call set_method, set_path, set_user_agent, and build.
    pub struct Http3RequestBuilder {
        method: Option<H3Method>,
        path: Option<&'static str>,
        scheme: Option<&'static str>,
        user_agent: Option<&'static str>,
        authority: Option<SocketAddr>,
        custom_headers: Option<Vec<(&'static str, &'static str)>>,
    }

    impl Http3RequestBuilder {
        pub fn post(
            &mut self,
            path: &'static str,
            data: Vec<u8>,
            body_type: BodyType,
        ) -> &mut Self {
            self.method = Some(H3Method::POST { data, body_type });
            self.path = Some(path);
            self
        }
        pub fn get(&mut self, path: &'static str) -> &mut Self {
            self.method = Some(H3Method::GET);
            self.path = Some(path);
            self
        }
        pub fn set_user_agent(&mut self, user_agent: &'static str) -> &mut Self {
            self.user_agent = Some(user_agent);
            self
        }
        pub fn set_header(&mut self, name: &'static str, value: &'static str) -> &mut Self {
            if let None = self.custom_headers {
                self.custom_headers = Some(vec![(name, value)]);
            } else {
                self.custom_headers.as_mut().unwrap().push((name, value));
            }
            self
        }
        pub fn build(&mut self) -> Result<(Vec<Http3Request>, Option<Http3RequestConfirm>), ()> {
            if self.method.is_none() || self.path.is_none() || self.authority.is_none() {
                debug!("http3 request, nothing to build !");
                return Err(());
            }
            let body = Some(vec![1000]);
            let (sender, receiver) = crossbeam::channel::bounded::<(u64, String)>(1);
            let confirmation = Some(Http3RequestConfirm { response: receiver });

            let http_request = match self.method.take().unwrap() {
                H3Method::GET => vec![Http3Request::Header(
                    HeaderRequest::new(true, sender)
                        .add_header(":method", "GET")
                        .add_header(":scheme", "https")
                        .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                        .add_header(
                            ":authority",
                            self.authority.as_ref().unwrap().to_string().as_str(),
                        )
                        .add_header(
                            "user-agent",
                            self.user_agent.as_ref().unwrap().to_string().as_str(),
                        )
                        .add_header("accept", "*/*"),
                )],
                H3Method::POST {
                    mut data,
                    body_type,
                } => vec![
                    Http3Request::Header(
                        HeaderRequest::new(false, sender)
                            .add_header(":method", "POST")
                            .add_header(":scheme", "https")
                            .add_header(":path", self.path.as_ref().unwrap().to_string().as_str())
                            .add_header("content-length", data.len().to_string().as_str())
                            .add_header(
                                ":authority",
                                self.authority.as_ref().unwrap().to_string().as_str(),
                            )
                            .add_header(
                                "user-agent",
                                self.user_agent.as_ref().unwrap().to_string().as_str(),
                            )
                            .add_header("accept", "*/*"),
                    ),
                    //99 is a place holder before receive a real stream id after submitting the
                    //   header request
                    Http3Request::Body(BodyRequest::new(
                        99,
                        1,
                        std::mem::replace(&mut data, vec![]),
                        true,
                    )),
                ],
                _ => vec![],
            };

            if !http_request.is_empty() {
                Ok((http_request, confirmation))
            } else {
                Err(())
            }
        }
    }
}

mod request_format {

    use super::*;
    use quiche::h3;
    #[derive(PartialEq)]
    pub enum H3Method {
        GET,
        POST { data: Vec<u8>, body_type: BodyType },
        PUT,
        DELETE,
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
                    data: vec![],
                    body_type: BodyType::Ping,
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
        let mut new_request: Http3RequestBuilder =
            Http3Request::new(Some(SocketAddr::from_str("127.0.0.1:3000").unwrap()));

        let data = vec![0; 5000];
        let data_len = data.len();
        let body_type = BodyType::Ping;
        let request = new_request
            .post("/post", data, body_type)
            .set_user_agent("Camille")
            .build();

        assert!(request.is_ok());

        let request = request.unwrap();

        assert!(!request.0.is_empty());
        assert!(request.1.is_some());

        // Get method build only 1 request header
        assert!(request.0.len() == 2);

        let header_r = &request.0[0];
        let body_r = &request.0[1];
        match header_r {
            Http3Request::Header(header) => {
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
            Http3Request::Body(_) => {
                assert!(false)
            }
            Http3Request::BodyFromFile => {}
        }
    }
    #[test]
    fn build_get_request() {
        let mut new_request: Http3RequestBuilder =
            Http3Request::new(Some(SocketAddr::from_str("127.0.0.1:3000").unwrap()));

        let request = new_request.get("/path").set_user_agent("Camille").build();

        assert!(request.is_ok());

        let request = request.unwrap();

        assert!(!request.0.is_empty());
        assert!(request.1.is_some());

        // Get method build only 1 request header
        assert!(request.0.len() == 1);

        let http_request: &Http3Request = &request.0[0];
        match http_request {
            Http3Request::Header(header) => {
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
            Http3Request::Body(_) => {
                assert!(false)
            }
            Http3Request::BodyFromFile => {}
        }
    }
}
