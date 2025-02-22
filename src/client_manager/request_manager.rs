#![allow(warnings)]
pub use queue_builder::{RequestChannel, RequestHead, RequestQueue};
pub use request_builder::{Http3Request, Http3RequestBuilder, Http3RequestConfirm};
mod queue_builder {
    use quiche::h3::Header;

    use super::*;

    pub struct RequestChannel {
        channel: (
            crossbeam::channel::Sender<Http3Request>,
            crossbeam::channel::Receiver<Http3Request>,
        ),
    }

    pub struct RequestHead {
        head: crossbeam::channel::Sender<Http3Request>,
    }
    impl RequestHead {
        pub fn send_request(
            &self,
            request: Http3Request,
        ) -> Result<(), crossbeam::channel::SendError<Http3Request>> {
            self.head.send(request)
        }
    }

    pub struct RequestQueue {
        queue: crossbeam::channel::Receiver<Http3Request>,
    }

    impl RequestQueue {
        pub fn pop_request(
            &self,
            connexion_id: String,
            req_cb: impl FnOnce(&Vec<Header>) -> Result<u64, quiche::h3::Error>,
        ) {
            if let Ok(new_req) = self.queue.try_recv() {
                if let Ok(stream_id) = req_cb(new_req.headers()) {
                    new_req.send_ids(stream_id, connexion_id.as_str());
                }
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
    pub struct Http3Request {
        headers: Vec<h3::Header>,
        stream_id_response: crossbeam::channel::Sender<(u64, String)>,
    }

    impl Http3Request {
        pub fn new() -> Http3RequestBuilder {
            Http3RequestBuilder {
                method: None,
                path: None,
                scheme: None,
                user_agent: None,
            }
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
    }

    pub struct Http3RequestBuilder {
        method: Option<H3Method>,
        path: Option<&'static str>,
        scheme: Option<&'static str>,
        user_agent: Option<&'static str>,
    }

    impl Http3RequestBuilder {
        pub fn build(&self) -> Result<(Http3Request, Http3RequestConfirm), ()> {
            if self.method.is_none() || self.path.is_none() {
                println!("http3 request, nothing to build !");
                return Err(());
            }
            let (sender, receiver) = crossbeam::channel::bounded::<(u64, String)>(1);
            let confirmation = Http3RequestConfirm { response: receiver };
            Ok((
                Http3Request {
                    headers: vec![],
                    stream_id_response: sender,
                },
                confirmation,
            ))
        }
    }
}

mod request_format {

    use super::*;
    use quiche::h3;
    #[derive(PartialEq)]
    pub enum H3Method {
        GET,
        POST,
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
                "POST" => Ok(H3Method::POST),
                "PUT" => Ok(H3Method::PUT),
                "DELETE" => Ok(H3Method::DELETE),
                &_ => Err(()),
            }
        }
    }

    #[derive(PartialEq)]
    pub enum RequestType {
        Ping,
        Message(String),
        File(String),
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
        request_type: RequestType,
    }

    impl PartialEq for RequestForm {
        fn eq(&self, other: &Self) -> bool {
            self.method() == other.method()
                && self.path() == self.path()
                && self.scheme == self.scheme
                && self.authority == other.authority
                && self.request_type() == other.request_type()
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
        pub fn request_type(&self) -> &RequestType {
            &self.request_type
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
        request_type: Option<RequestType>,
    }

    impl RequestFormBuilder {
        pub fn new() -> Self {
            Self {
                method: None,
                path: None,
                scheme: None,
                authority: None,
                body_cb: None,
                request_type: None,
            }
        }

        pub fn build(&mut self) -> RequestForm {
            RequestForm {
                method: self.method.take().unwrap(),
                path: self.path.take().unwrap(),
                scheme: self.scheme.take().expect("expected scheme"),
                authority: self.authority.clone(),
                body_cb: self.body_cb.take().expect("No callback cb set"),
                request_type: self.request_type.take().unwrap(),
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
        pub fn set_request_type(&mut self, request_type: RequestType) -> &mut Self {
            self.request_type = Some(request_type);
            self
        }
    }
}
