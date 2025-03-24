mod body_manager;
mod client_request_manager;
mod request_manager;
mod response_manager;
pub use body_manager::{BodyChannel, BodyHead, BodyQueue};
pub use client_request_manager::ClientRequestManager;
pub use request_manager::{
    BodyType, ContentType, H3Method, Http3Request, ProgressTracker, RequestChannel, RequestEvent,
    RequestEventListener, RequestQueue,
};
pub use response_manager::{
    Http3Response, ResponseChannel, ResponseHead, ResponseQueue, UploadProgressStatus,
};

pub use super::client_config::ConnexionInfos;
use super::client_init::Http3Client;
pub use client_management::Http3ClientManager;

mod client_management {

    use std::sync::Arc;

    use crate::client_config::{self, ClientConfig};

    use self::{request_manager::Http3RequestBuilder, response_manager::WaitPeerResponse};

    use super::*;

    pub struct Http3ClientManager {
        request_channel: RequestChannel,
        response_channel: ResponseChannel,
        body_channel: BodyChannel,
        request_manager: ClientRequestManager,
        connexion_infos: ConnexionInfos,
        http3_client: Arc<Http3Client>,
    }

    impl Clone for Http3ClientManager {
        fn clone(&self) -> Self {
            Self {
                request_channel: self.request_channel.clone(),
                response_channel: self.response_channel.clone(),
                body_channel: self.body_channel.clone(),
                request_manager: self.request_manager.clone(),
                connexion_infos: self.connexion_infos.clone(),
                http3_client: self.http3_client.clone(),
            }
        }
    }

    impl Http3ClientManager {
        ///
        ///Create the Http3ClientManager instance. It is the main interface to the quiche client.
        ///
        ///
        ///ConnexionInfos can be modified with new_connect_infos()
        ///
        pub fn new(peer_socket_address: &str) -> Self {
            let client_config = ClientConfig::new();
            client_config
                .connexion_infos()
                .set_peer_address(peer_socket_address)
                .set_local_address("0.0.0.0:0")
                .build_connexion_infos();
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
        pub fn builder() -> () {
            ()
        }

        pub fn new_request(
            &self,
            request_builder: impl FnOnce(&mut Http3RequestBuilder),
        ) -> Result<WaitPeerResponse, ()> {
            self.request_manager.new_request(request_builder)
        }

        pub fn new_connect_infos(&self, new_client_config: ClientConfig) -> &Self {
            self.connexion_infos
                .update(&new_client_config.connexion_infos());
            self
        }
        pub fn connexion_infos(&self) -> &ConnexionInfos {
            &self.connexion_infos
        }

        /// Handle to ClientRequestManager. Send new request from it.
        pub fn request_manager(&self) -> ClientRequestManager {
            self.request_manager.clone()
        }

        /*
                fn request_head(&self) -> RequestHead {
                    self.request_channel.get_head()
                }
                fn response_head(&self) -> ResponseHead {
                    self.response_channel.get_head()
                }

                fn request_queue(&self) -> RequestQueue {
                    self.request_channel.get_queue()
                }
                fn response_queue(&self) -> ResponseQueue {
                    self.response_channel.get_queue()
                }
                fn body_head(&self) -> BodyHead {
                    self.body_channel.get_head()
                }
                fn body_queue(&self) -> BodyQueue {
                    self.body_channel.get_queue()
                }
        */
    }
}
