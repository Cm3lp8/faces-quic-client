pub use queue_builder::{RequestChannel, RequestHead, RequestQueue};
pub use request_builder::Http3Request;
mod queue_builder {
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

    pub struct RequestQueue {
        queue: crossbeam::channel::Receiver<Http3Request>,
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
    use quiche::h3;

    use super::*;

    pub struct Http3RequestConfirm {
        response: crossbeam::channel::Receiver<u64>,
    }

    pub struct Http3Request {
        headers: Vec<h3::Header>,
        stream_id_response: crossbeam::channel::Sender<u64>,
    }

    impl Http3Request {
        pub fn new() -> Http3RequestBuilder {
            Http3RequestBuilder
        }
    }

    pub struct Http3RequestBuilder;

    impl Http3RequestBuilder {
        pub fn build(&self) -> (Http3Request, Http3RequestConfirm) {
            let (sender, receiver) = crossbeam::channel::bounded::<u64>(1);
            let confirmation = Http3RequestConfirm { response: receiver };
            (
                Http3Request {
                    headers: vec![],
                    stream_id_response: sender,
                },
                confirmation,
            )
        }
    }
}
