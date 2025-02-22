pub use client_request_mngr::ClientRequestManager;

mod client_request_mngr {
    use std::sync::Arc;

    use crate::{
        client_config::ConnexionInfos,
        client_init::Http3Client,
        client_manager::{
            request_manager::{
                Http3Request, Http3RequestBuilder, Http3RequestConfirm, RequestHead,
            },
            response_manager::{Http3Response, PartialResponse, ResponseManager, WaitPeerResponse},
            BodyHead, RequestQueue, ResponseQueue,
        },
    };

    use super::*;

    ///
    ///Interface with the client. Create a new request, send data from here.
    ///
    pub struct ClientRequestManager {
        request_head: RequestHead,
        response_queue: ResponseQueue,
        body_head: BodyHead,
        connexion_infos: ConnexionInfos,
        response_manager: ResponseManager,
        http3_client: Arc<Http3Client>,
    }

    impl Clone for ClientRequestManager {
        fn clone(&self) -> Self {
            Self {
                request_head: self.request_head.clone(),
                response_queue: self.response_queue.clone(),
                body_head: self.body_head.clone(),
                connexion_infos: self.connexion_infos.clone(),
                response_manager: self.response_manager.clone(),
                http3_client: self.http3_client.clone(),
            }
        }
    }

    impl ClientRequestManager {
        pub fn new(
            request_head: RequestHead,
            response_queue: ResponseQueue,
            body_head: BodyHead,
            connexion_infos: ConnexionInfos,
            http3_client: Arc<Http3Client>,
        ) -> Self {
            let resp_queue = response_queue.clone();
            let response_manager = ResponseManager::new(resp_queue);
            response_manager.run();

            Self {
                request_head,
                response_queue,
                body_head,
                connexion_infos,
                response_manager,
                http3_client,
            }
        }

        ///
        ///create a new response, get a lazy Http3Response as Result (lazy : response fetching
        ///can retrieve when user want)
        ///
        pub fn new_request(
            &mut self,
            request_builder: impl FnOnce(&mut Http3RequestBuilder),
        ) -> Result<WaitPeerResponse, ()> {
            let mut http3_request_builder =
                Http3Request::new(self.connexion_infos.get_peer_socket_address());
            request_builder(&mut http3_request_builder);

            match http3_request_builder.build() {
                Ok((http3_request, http3_confirm)) => {
                    /*
                     *
                     * if connexion is closed, open it :
                     *
                     * */
                    if self.http3_client.is_off() {
                        if let Ok(_) = self.http3_client.connect() {}
                    }

                    for req in &http3_request {
                        match req {
                            Http3Request::Header(header_req) => {
                                self.request_head
                                    .send_request(Http3Request::Header(header_req.clone()))
                                    .unwrap();
                            }
                            _ => {}
                        }
                    }
                    let stream_ids = http3_confirm.unwrap().wait_stream_ids();
                    let stream_id = stream_ids.as_ref().unwrap().0;

                    for req in http3_request {
                        match req {
                            Http3Request::Body(mut body_req) => {
                                self.request_head
                                    .send_body(stream_id, 4096, body_req.take_data());
                            }
                            _ => {}
                        }
                    }

                    let response_manager_submission = self.response_manager.submitter();
                    let response_chan = crossbeam::channel::bounded::<WaitPeerResponse>(1);
                    let response_sender = response_chan.0.clone();

                    std::thread::spawn(move || {
                        /*
                         *
                         * wait here the stream_id with the response confirm
                         *
                         * */
                        if let Ok(stream_ids) = stream_ids {
                            let (partial_response, completed_channel) =
                                PartialResponse::new(&stream_ids);

                            let peer_response =
                                WaitPeerResponse::new(&stream_ids, completed_channel);
                            if let Err(e) = response_sender.send(peer_response) {
                                println!("Error: sending back WaitPeerResponse failed stream_id[{:?}] [{:?}]",stream_ids,e);
                            }

                            //send partial response to the reponse manager
                            if let Err(e) = response_manager_submission.submit(partial_response) {
                                println!("Error: failed to submit Partial response for stream_id[{:?}]   [{:?}]", stream_ids,e );
                            }
                        }

                        /*
                         *
                         *
                         * Get the response back -> ask the response table in the response worker
                         * with the stream_id that is unique per connexion
                         *
                         *
                         * */
                    });

                    if let Ok(response) = response_chan.1.recv() {
                        Ok(response)
                    } else {
                        Err(())
                    }
                }
                Err(()) => Err(()),
            }
        }
    }
}
