mod body_manager;
mod client_request_manager;
mod request_manager;
mod response_manager;
pub use body_manager::{BodyChannel, BodyHead, BodyQueue};
pub use request_manager::{RequestChannel, RequestQueue};
pub use response_manager::{ResponseChannel, ResponseHead, ResponseQueue};

mod client_management {

    use self::{
        client_request_manager::ClientRequestManager, request_manager::RequestHead,
        response_manager::ResponseHead,
    };

    use super::*;

    pub struct Http3ClientManager {
        request_channel: RequestChannel,
        response_channel: ResponseChannel,
        body_channel: BodyChannel,
        request_manager: ClientRequestManager,
    }

    impl Http3ClientManager {
        ///
        ///Create the Http3ClientManager instance on which request and response channels (head and queue) can be called, as
        ///well as communication interface with the hosting app
        ///
        pub fn new() -> Self {
            let request_channel = RequestChannel::new();
            let response_channel = ResponseChannel::new();
            let body_channel = BodyChannel::new();
            let request_manager = ClientRequestManager::new(
                request_channel.get_head(),
                response_channel.get_queue(),
                body_channel.get_head(),
            );

            Self {
                request_channel,
                response_channel,
                body_channel,
                request_manager,
            }
        }

        pub fn request_head(&self) -> RequestHead {
            self.request_channel.get_head()
        }
        pub fn response_head(&self) -> ResponseHead {
            self.response_channel.get_head()
        }

        pub fn request_queue(&self) -> RequestQueue {
            self.request_channel.get_queue()
        }
        pub fn response_queue(&self) -> ResponseQueue {
            self.response_channel.get_queue()
        }
        pub fn body_head(&self) -> BodyHead {
            self.body_channel.get_head()
        }
        pub fn body_queue(&self) -> BodyQueue {
            self.body_channel.get_queue()
        }
    }
}
