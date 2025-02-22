mod body_manager;
mod client_request_manager;
mod request_manager;
mod response_manager;
pub use body_manager::{BodyChannel, BodyHead, BodyQueue};
pub use client_request_manager::ClientRequestManager;
pub use request_manager::{Http3Request, RequestChannel, RequestQueue};
pub use response_manager::{Http3Response, ResponseChannel, ResponseHead, ResponseQueue};

pub use super::client_config::ConnexionInfos;
use super::client_init::Http3Client;
pub use client_management::Http3ClientManager;

mod client_management {

    use std::sync::Arc;

    use crate::client_config::ClientConfig;

    use self::{request_manager::RequestHead, response_manager::ResponseHead};

    use super::*;

    pub struct Http3ClientManager {
        request_channel: RequestChannel,
        response_channel: ResponseChannel,
        body_channel: BodyChannel,
        request_manager: ClientRequestManager,
        connexion_infos: ConnexionInfos,
        http3_client: Arc<Http3Client>,
    }

    impl Http3ClientManager {
        ///
        ///Create the Http3ClientManager instance on which request and response channels (head and queue) can be called, as
        ///well as communication interface with the hosting app
        ///
        ///ConnexionInfos can be modified with new_connect_infos()
        ///
        pub fn new(client_config: ClientConfig) -> Self {
            let request_channel = RequestChannel::new();
            let response_channel = ResponseChannel::new();
            let body_channel = BodyChannel::new();
            let http3_client = Http3Client::new(
                client_config.clone(),
                request_channel.get_queue(),
                response_channel.get_head(),
                body_channel.get_queue(),
            );
            let http3_client_arc = Arc::new(http3_client);
            let http3_client_arc_clone = http3_client_arc.clone();
            let request_manager = ClientRequestManager::new(
                request_channel.get_head(),
                response_channel.get_queue(),
                body_channel.get_head(),
                client_config.connexion_infos(),
                http3_client_arc,
            );

            Self {
                request_channel,
                response_channel,
                body_channel,
                request_manager,
                connexion_infos: client_config.connexion_infos(),
                http3_client: http3_client_arc_clone,
            }
        }

        pub fn new_connect_infos(&self, new_client_config: ClientConfig) -> &Self {
            self.connexion_infos
                .update(&new_client_config.connexion_infos());
            self
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
