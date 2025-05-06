mod body_manager;
mod client_request_manager;
mod persistant_stream;
mod request_manager;
mod response_manager;
pub use body_manager::{BodyChannel, BodyHead, BodyQueue};
pub use client_request_manager::ClientRequestManager;
pub use request_manager::{
    BodyType, ContentType, H3Method, Http3Request, ProgressTracker, RequestChannel, RequestEvent,
    RequestEventListener, RequestQueue,
};
pub use response_manager::{
    Http3Response, ReqStatus, ResponseChannel, ResponseHead, ResponseQueue, UploadProgressStatus,
};

pub use super::client_config::ConnexionInfos;
use super::client_init::Http3Client;
pub use client_management::Http3ClientManager;

mod client_management {

    use std::{
        collections::HashMap,
        hash::Hash,
        path::Path,
        sync::{Arc, Mutex},
    };

    use log::{error, info};
    use ring::error;
    use uuid::Uuid;

    use crate::{
        client_config::{self, ClientConfig},
        client_traits::IntoBodyReq,
        my_log,
    };

    use self::{
        persistant_stream::{StreamBuilder, StreamEvent},
        request_manager::{Http3RequestBuilder, Http3RequestPrep},
        response_manager::WaitPeerResponse,
    };

    use super::*;

    pub struct Http3ClientManager {
        request_channel: RequestChannel,
        response_channel: ResponseChannel,
        body_channel: BodyChannel,
        request_manager: ClientRequestManager,
        request_builder: Arc<Mutex<HashMap<Uuid, Http3RequestBuilder>>>,
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
                request_builder: Arc::new(Mutex::new(HashMap::new())),
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
            my_log::init();
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
            let request_builder: Arc<Mutex<HashMap<Uuid, Http3RequestBuilder>>> =
                Arc::new(Mutex::new(HashMap::new()));
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
                request_builder,
                connexion_infos: client_config.connexion_infos(),
                http3_client: http3_client_arc_clone,
            }
        }

        pub fn builder() -> () {
            ()
        }

        pub fn request_manager_ref(&self) -> &ClientRequestManager {
            &self.request_manager
        }
        pub fn get_stream(&self, path: &str) -> ReqBuilderOutput {
            let reqbuild_uuid = uuid::Uuid::new_v4();
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                reqbuild_uuid,
            );

            http3_request_builder.get_stream(path.to_owned());

            self.request_builder
                .lock()
                .unwrap()
                .entry(reqbuild_uuid)
                .insert_entry(http3_request_builder);
            ReqBuilderOutput(reqbuild_uuid, self)
        }

        pub fn get(&self, path: &str) -> ReqBuilderOutput {
            let reqbuild_uuid = uuid::Uuid::new_v4();
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                reqbuild_uuid,
            );
            http3_request_builder.get(path.to_owned());

            self.request_builder
                .lock()
                .unwrap()
                .entry(reqbuild_uuid)
                .insert_entry(http3_request_builder);

            ReqBuilderOutput(reqbuild_uuid, self)
        }
        pub fn post_data(&self, path: &str, data: impl IntoBodyReq) -> ReqBuilderOutput {
            let reqbuild_uuid = uuid::Uuid::new_v4();
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                reqbuild_uuid,
            );
            let content_type = data.content_type();
            http3_request_builder
                .post_data(path.to_string(), data.into_bytes())
                .set_content_type(content_type);

            self.request_builder
                .lock()
                .unwrap()
                .entry(reqbuild_uuid)
                .insert_entry(http3_request_builder);

            ReqBuilderOutput(reqbuild_uuid, self)
        }
        pub fn post_file(&self, path: String, file_path: impl AsRef<Path>) -> ReqBuilderOutput {
            let reqbuild_uuid = uuid::Uuid::new_v4();
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                reqbuild_uuid,
            );
            http3_request_builder.post_file(path, file_path);

            self.request_builder
                .lock()
                .unwrap()
                .entry(reqbuild_uuid)
                .insert_entry(http3_request_builder);

            ReqBuilderOutput(reqbuild_uuid, self)
        }
        pub fn delete(&self, path: String, auth_token: String) -> ReqBuilderOutput {
            let reqbuild_uuid = uuid::Uuid::new_v4();
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                reqbuild_uuid,
            );
            http3_request_builder.delete(path, auth_token);

            self.request_builder
                .lock()
                .unwrap()
                .entry(reqbuild_uuid)
                .insert_entry(http3_request_builder);

            ReqBuilderOutput(reqbuild_uuid, self)
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

    pub struct ReqBuilderOutput<'a>(Uuid, &'a Http3ClientManager);

    impl<'a> ReqBuilderOutput<'a> {
        pub fn send(&self) -> Result<WaitPeerResponse, ()> {
            let uuid = self.0;

            if let Some(entry) = self.1.request_builder.lock().unwrap().get_mut(&uuid) {
                return self.1.request_manager.new_request_with_builder(entry);
            }
            Err(())
        }
        pub fn stream(&self) -> StreamBuilder {
            let uuid = self.0;
            StreamBuilder::with_request_map(uuid, self.1.request_builder.clone(), self.1)
        }
        pub fn header(&self, name: &str, value: &str) -> &Self {
            let uuid = self.0;
            if let Some(entry) = self.1.request_builder.lock().unwrap().get_mut(&uuid) {
                entry.set_header(name.to_string(), value.to_string());
            }
            self
        }
        pub fn set_user_agent(&self, user_agent: &str) -> &Self {
            let uuid = self.0;

            if let Some(entry) = self.1.request_builder.lock().unwrap().get_mut(&uuid) {
                entry.set_user_agent(user_agent.to_string());
            }

            self
        }
        pub fn subscribe_event(
            &self,
            event_listener: Arc<dyn RequestEventListener + 'static + Send + Sync>,
        ) -> &Self {
            let uuid = self.0;

            if let Some(entry) = self.1.request_builder.lock().unwrap().get_mut(&uuid) {
                entry.subscribe_event(event_listener);
            }

            self
        }
    }
}
