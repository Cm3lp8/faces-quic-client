pub use client_request_mngr::ClientRequestManager;
mod client_request_mngr {
    use crate::client_manager::{
        request_manager::RequestHead, BodyHead, RequestQueue, ResponseQueue,
    };

    use super::*;

    ///
    ///Interface with the client. Create a new request, send data from here.
    ///
    pub struct ClientRequestManager {
        request_head: RequestHead,
        response_queue: ResponseQueue,
        body_head: BodyHead,
    }

    impl Clone for ClientRequestManager {
        fn clone(&self) -> Self {
            Self {
                request_head: self.request_head.clone(),
                response_queue: self.response_queue.clone(),
                body_head: self.body_head.clone(),
            }
        }
    }

    impl ClientRequestManager {
        pub fn new(
            request_head: RequestHead,
            response_queue: ResponseQueue,
            body_head: BodyHead,
        ) -> Self {
            Self {
                request_head,
                response_queue,
                body_head,
            }
        }

        pub fn new_request(&mut self) {}
    }
}
