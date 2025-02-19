pub use client_request_mngr::ClientRequestManager;
mod client_request_mngr {
    use crate::{
        client_config::ConnexionInfos,
        client_manager::{
            request_manager::{
                Http3Request, Http3RequestBuilder, Http3RequestConfirm, RequestHead,
            },
            response_manager::Http3Response,
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
    }

    impl Clone for ClientRequestManager {
        fn clone(&self) -> Self {
            Self {
                request_head: self.request_head.clone(),
                response_queue: self.response_queue.clone(),
                body_head: self.body_head.clone(),
                connexion_infos: self.connexion_infos.clone(),
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
            Self {
                request_head,
                response_queue,
                body_head,
                connexion_infos,
            }
        }

        ///
        ///create a new response, get a lazy Http3Response as Result (lazy : response fetching
        ///can retrieve when user want)
        ///
        pub fn new_request(
            &mut self,
            request_builder: impl FnOnce(&mut Http3RequestBuilder),
        ) -> Result<Http3Response, ()> {
            let mut http3_request_builder = Http3Request::new();
            request_builder(&mut http3_request_builder);

            match http3_request_builder.build() {
                Ok((http3_request, http3_confirm)) => {
                    self.request_head.send_request(http3_request).unwrap();
                    /*
                     *
                     * wait here the stream_id with the response confirm
                     *
                     * */

                    std::thread::spawn(move || {
                        let stream_id = http3_confirm.wait_stream_id();

                        /*
                         *
                         *
                         * Get the response back -> ask the response table in the response worker
                         * with the stream_id that is unique per connexion
                         *
                         *
                         * */
                    });

                    Ok(http3_response)
                }
                Err(()) => Err(()),
            }
        }
    }
}
