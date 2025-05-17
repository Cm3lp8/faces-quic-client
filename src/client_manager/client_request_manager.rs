pub use client_request_mngr::ClientRequestManager;

mod client_request_mngr {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use log::{error, info, warn};
    use mio::Waker;
    use uuid::Uuid;

    use crate::{
        client_config::ConnexionInfos,
        client_init::Http3Client,
        client_manager::{
            persistant_stream::{
                KeepAlive, PingEmitter, StreamControlFlow, StreamEvent, StreamSub,
            },
            request_manager::{Http3Request, Http3RequestBuilder, Http3RequestPrep, RequestHead},
            response_manager::{PartialResponse, ResponseManager, WaitPeerResponse},
            BodyHead, ResponseQueue,
        },
        my_log,
    };

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
        waker: Arc<Mutex<Option<Waker>>>,
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
                waker: self.waker.clone(),
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
                waker: Arc::new(Mutex::new(None)),
            }
        }
        pub fn wake_client(&self) {
            if let Some(waker) = &*self.waker.lock().unwrap() {
                if let Err(e) = waker.wake() {
                    println!("Error : failed waking up the client [{:?}]", e);
                }
            }
        }
        pub fn new_stream_with_builder(
            &self,
            http3_request_builder: &mut Http3RequestBuilder,
            keep_alive: &Option<KeepAlive>,
            stream_cb: impl Fn(StreamEvent, StreamControlFlow) + Send + Sync + 'static,
        ) -> Result<WaitPeerResponse, ()> {
            let path = http3_request_builder.get_path();
            my_log::debug("ici connexion ping");
            match http3_request_builder.build_down_stream(keep_alive) {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    /*
                     *
                     * if connexion is closed, open it :
                     *
                     * */
                    if self.http3_client.is_off() {
                        if let Ok((_conn_id, waker)) = self.http3_client.connect() {
                            *self.waker.lock().unwrap() = Some(waker);
                        }
                    }

                    // sending first header, waiting for a stream id
                    for req in &http3_request {
                        match req {
                            Http3RequestPrep::Header(header_req) => {
                                let adjust_sending_duration =
                                    crossbeam::channel::bounded::<Instant>(1);
                                if let Err(e) = self.request_head.send_request((
                                    Http3Request::Header(header_req.clone()),
                                    adjust_sending_duration.0,
                                )) {
                                    println!("Error sending header request [{:?}]", e)
                                } else {
                                    self.wake_client();
                                }
                            }
                            _ => {}
                        }
                    }
                    // Once the stream has been created, we received it back from client quiche
                    // loop;
                    let stream_ids = http3_confirm.unwrap().wait_stream_ids();
                    let stream_id = stream_ids.as_ref().unwrap().0;

                    for req in &http3_request {
                        match req {
                            Http3RequestPrep::Ping(duration) => {
                                let ping_stop = PingEmitter::run(
                                    *duration,
                                    &self.request_head,
                                    stream_id,
                                    &self.waker,
                                );
                            }

                            _ => {}
                        }
                    }
                    for req in http3_request {
                        match req {
                            Http3RequestPrep::Body(body_req) => {
                                my_log::debug(&body_req);
                                self.request_head
                                    .send_body(stream_id, 8192, body_req.take());
                            }
                            _ => my_log::log("no body"),
                        }
                    }

                    let response_manager_submission = self.response_manager.submitter();
                    let response_chan = crossbeam::channel::bounded::<WaitPeerResponse>(1);
                    let response_sender = response_chan.0.clone();
                    let stream_cb_syncable = Arc::new(stream_cb);

                    std::thread::spawn(move || {
                        /*
                         *
                         * wait here the stream_id with the response confirm
                         *
                         * */
                        if let Ok(stream_ids) = stream_ids {
                            let (partial_response, completed_channel, progress_channel) =
                                PartialResponse::new_streamable(
                                    path.unwrap().as_str(),
                                    event_subscriber,
                                    StreamSub::Downstream(stream_cb_syncable),
                                    &stream_ids,
                                );

                            let peer_response = WaitPeerResponse::new(
                                &stream_ids,
                                completed_channel,
                                progress_channel,
                            );
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
        pub fn new_request_with_builder(
            &self,
            http3_request_builder: &mut Http3RequestBuilder,
        ) -> Result<WaitPeerResponse, ()> {
            let path = http3_request_builder.get_path();
            match http3_request_builder.build() {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    /*
                     *
                     * if connexion is closed, open it :
                     *
                     * */
                    if self.http3_client.is_off() {
                        if let Ok((_conn_id, waker)) = self.http3_client.connect() {
                            *self.waker.lock().unwrap() = Some(waker);
                        }
                    }

                    for req in &http3_request {
                        match req {
                            Http3RequestPrep::Header(header_req) => {
                                let adjust_sending_duration =
                                    crossbeam::channel::bounded::<Instant>(1);
                                if let Err(e) = self.request_head.send_request((
                                    Http3Request::Header(header_req.clone()),
                                    adjust_sending_duration.0,
                                )) {
                                    println!("Error sending header request [{:?}]", e)
                                } else {
                                    self.wake_client();
                                }
                            }
                            _ => {}
                        }
                    }
                    let stream_ids = http3_confirm.unwrap().wait_stream_ids();
                    let stream_id = stream_ids.as_ref().unwrap().0;

                    for req in http3_request {
                        match req {
                            Http3RequestPrep::Body(body_req) => {
                                my_log::debug(&body_req);
                                self.request_head
                                    .send_body(stream_id, 8192, body_req.take());
                            }
                            _ => my_log::log("no body"),
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
                            let (partial_response, completed_channel, progress_channel) =
                                PartialResponse::new(
                                    path.unwrap().as_str(),
                                    event_subscriber,
                                    &stream_ids,
                                );

                            let peer_response = WaitPeerResponse::new(
                                &stream_ids,
                                completed_channel,
                                progress_channel,
                            );
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

        ///
        ///create a new http3 request. Returns a lazy Http3Response as Result (lazy : response fetching
        ///can retrieve when user want with recv call).
        ///
        pub fn new_request(
            &self,
            request_builder: impl FnOnce(&mut Http3RequestBuilder),
        ) -> Result<WaitPeerResponse, ()> {
            let mut http3_request_builder = Http3RequestPrep::new(
                self.connexion_infos.get_peer_socket_address(),
                Uuid::new_v4(),
            );
            request_builder(&mut http3_request_builder);

            let path = http3_request_builder.get_path();
            match http3_request_builder.build() {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    /*
                     *
                     * if connexion is closed, open it :
                     *
                     * */
                    if self.http3_client.is_off() {
                        if let Ok((_conn_id, waker)) = self.http3_client.connect() {
                            *self.waker.lock().unwrap() = Some(waker);
                        }
                    }

                    for req in &http3_request {
                        match req {
                            Http3RequestPrep::Header(header_req) => {
                                let adjust_sending_duration =
                                    crossbeam::channel::bounded::<Instant>(1);
                                if let Err(e) = self.request_head.send_request((
                                    Http3Request::Header(header_req.clone()),
                                    adjust_sending_duration.0,
                                )) {
                                    println!("Error sending header request [{:?}]", e)
                                } else {
                                    self.wake_client();
                                }
                            }
                            _ => {}
                        }
                    }
                    let stream_ids = http3_confirm.unwrap().wait_stream_ids();
                    let stream_id = stream_ids.as_ref().unwrap().0;

                    for req in http3_request {
                        match req {
                            Http3RequestPrep::Body(body_req) => {
                                self.request_head
                                    .send_body(stream_id, 8192, body_req.take());
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
                            let (partial_response, completed_channel, progress_channel) =
                                PartialResponse::new(
                                    path.unwrap().as_str(),
                                    event_subscriber,
                                    &stream_ids,
                                );

                            let peer_response = WaitPeerResponse::new(
                                &stream_ids,
                                completed_channel,
                                progress_channel,
                            );
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
