pub use client_request_mngr::ClientRequestManager;
mod client_request_mngr {
    use crate::{
        client_config::ConnexionInfos,
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
    }

    impl Clone for ClientRequestManager {
        fn clone(&self) -> Self {
            Self {
                request_head: self.request_head.clone(),
                response_queue: self.response_queue.clone(),
                body_head: self.body_head.clone(),
                connexion_infos: self.connexion_infos.clone(),
                response_manager: self.response_manager.clone(),
            }
        }
    }

    impl ClientRequestManager {
        pub fn new(
            request_head: RequestHead,
            response_queue: ResponseQueue,
            body_head: BodyHead,
            connexion_infos: ConnexionInfos,
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
            let mut http3_request_builder = Http3Request::new();
            request_builder(&mut http3_request_builder);

            match http3_request_builder.build() {
                Ok((http3_request, http3_confirm)) => {
                    self.request_head.send_request(http3_request).unwrap();
                    /*
                     *
                     * wait here the stream_id with the response confirm
                     *
                     *
                     * */

                    let response_manager_submission = self.response_manager.submitter();
                    let response_chan = crossbeam::channel::bounded::<WaitPeerResponse>(1);
                    let response_sender = response_chan.0.clone();

                    std::thread::spawn(move || {
                        let stream_id = http3_confirm.wait_stream_id();
                        if let Ok(stream_id) = stream_id {
                            let (partial_response, completed_channel) =
                                PartialResponse::new(stream_id);

                            let peer_response = WaitPeerResponse::new(stream_id, completed_channel);
                            if let Err(e) = response_sender.send(peer_response) {
                                println!("Error: sending back WaitPeerResponse failed stream_id[{stream_id}] [{:?}]",e);
                            }

                            //send partial response to the reponse manager
                            if let Err(e) = response_manager_submission.submit(partial_response) {
                                println!("Error: failed to submit Partial response for stream_id[{stream_id}]   [{:?}]",e );
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
