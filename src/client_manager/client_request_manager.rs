pub use client_request_mngr::ClientRequestManager;

mod client_request_mngr {
    use std::{
        collections::HashMap,
        sync::{atomic::AtomicU64, Arc, Mutex},
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
            request_manager::{
                Http3Request, Http3RequestBuilder, Http3RequestConfirm, Http3RequestPrep,
                RequestHead,
            },
            response_manager::{PartialResponse, ResponseManager, WaitPeerResponse},
            BodyHead, ResponseQueue,
        },
        my_log,
        thread_controller::{self, ThreadController},
    };

    const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const FOREGROUND_CONNECT_TIMEOUT: Duration = Duration::from_millis(2500);
    const BACKGROUND_CONNECT_TIMEOUT: Duration = Duration::from_millis(2500);

    ///
    ///Interface with the client. Create a new request, send data from here.
    ///
    pub struct ClientRequestManager {
        stream_id_counter: Arc<AtomicU64>,
        request_head: RequestHead,
        response_queue: ResponseQueue,
        body_head: BodyHead,
        connexion_infos: ConnexionInfos,
        response_manager: ResponseManager,
        http3_client: Arc<Http3Client>,
        waker: Arc<Mutex<Option<Waker>>>,
        connect_lock: Arc<Mutex<()>>,
        thread_controller: ThreadController,
    }

    impl Clone for ClientRequestManager {
        fn clone(&self) -> Self {
            Self {
                stream_id_counter: self.stream_id_counter.clone(),
                request_head: self.request_head.clone(),
                response_queue: self.response_queue.clone(),
                body_head: self.body_head.clone(),
                connexion_infos: self.connexion_infos.clone(),
                response_manager: self.response_manager.clone(),
                http3_client: self.http3_client.clone(),
                waker: self.waker.clone(),
                connect_lock: self.connect_lock.clone(),
                thread_controller: self.thread_controller.clone(),
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
            thread_controller: &ThreadController,
            request_builder: &Arc<Mutex<HashMap<Uuid, Http3RequestBuilder>>>,
        ) -> Self {
            let resp_queue = response_queue.clone();
            let response_manager =
                ResponseManager::new(resp_queue, thread_controller, request_builder);
            response_manager.run();

            Self {
                stream_id_counter: Arc::new(AtomicU64::new(0)),
                request_head,
                response_queue,
                body_head,
                connexion_infos,
                response_manager,
                http3_client,
                waker: Arc::new(Mutex::new(None)),
                connect_lock: Arc::new(Mutex::new(())),
                thread_controller: thread_controller.clone(),
            }
        }
        pub fn stop(&self) {
            self.response_manager.stop();
        }
        pub fn rerun(&self) {
            self.response_manager.run();
        }
        pub fn close_stream(&self, stream_id: u64) -> Result<(), ()> {
            let adjust_sending_duration = crossbeam::channel::bounded::<Instant>(1);
            let res = self
                .request_head
                .send_request((
                    Http3Request::CloseStream { stream_id },
                    adjust_sending_duration.0,
                ))
                .map_err(|e| ());
            self.wake_client();
            res
        }
        pub fn close_connection(&self) -> Result<(), ()> {
            let adjust_sending_duration = crossbeam::channel::bounded::<Instant>(1);
            let stream_id = self
                .stream_id_counter
                .load(std::sync::atomic::Ordering::Relaxed);
            let res = self
                .request_head
                .send_request((
                    Http3Request::GoAway { stream_id },
                    adjust_sending_duration.0,
                ))
                .map_err(|e| ());
            self.wake_client();
            res
        }
        pub fn wake_client(&self) {
            if let Some(waker) = &*self.waker.lock().unwrap() {
                if let Err(e) = waker.wake() {
                    warn!(
                        "[faces_diag][quic_client][request] wake_failed error={:?}",
                        e
                    );
                    println!("Error : failed waking up the client [{:?}]", e);
                }
            } else {
                warn!("[faces_diag][quic_client][request] wake_skipped reason=no_waker");
            }
        }

        fn path_for_log(path: &Option<String>) -> &str {
            path.as_deref().unwrap_or("<unknown>")
        }

        fn should_flush_stale_queue(reason: &str) -> bool {
            matches!(reason, "app_resumed" | "network_available")
        }

        fn preconnect_timeout(reason: &str) -> Duration {
            match reason {
                "app_resumed" | "network_available" => FOREGROUND_CONNECT_TIMEOUT,
                _ => DEFAULT_CONNECT_TIMEOUT,
            }
        }

        fn request_connect_timeout(kind: &str, path: &Option<String>) -> Duration {
            match (kind, Self::path_for_log(path)) {
                ("downstream", _) | (_, "/down_stream_ack") => BACKGROUND_CONNECT_TIMEOUT,
                _ => DEFAULT_CONNECT_TIMEOUT,
            }
        }

        pub fn preconnect(&self, reason: &str) -> Result<(), ()> {
            let started_at = Instant::now();
            let was_off = self.http3_client.is_off();
            info!(
                "[faces_diag][quic_client][preconnect] begin reason={} connection_off={}",
                reason, was_off
            );

            if !was_off {
                info!(
                    "[faces_diag][quic_client][preconnect] reuse reason={} elapsed_ms={}",
                    reason,
                    started_at.elapsed().as_millis()
                );
                return Ok(());
            }

            let _connect_guard = self.connect_lock.lock().unwrap();
            if !self.http3_client.is_off() {
                info!(
                    "[faces_diag][quic_client][preconnect] reuse_after_wait reason={} elapsed_ms={}",
                    reason,
                    started_at.elapsed().as_millis()
                );
                return Ok(());
            }

            if Self::should_flush_stale_queue(reason) {
                let drained = self.http3_client.drain_stale_requests(reason);
                info!(
                    "[faces_diag][quic_client][preconnect] stale_queue_flush reason={} drained_requests={}",
                    reason, drained
                );
            }

            let timeout = Self::preconnect_timeout(reason);
            info!(
                "[faces_diag][quic_client][preconnect] start reason={} timeout_ms={}",
                reason,
                timeout.as_millis()
            );
            match self.http3_client.connect_with_timeout(timeout) {
                Ok((conn_id, waker)) => {
                    *self.waker.lock().unwrap() = Some(waker);
                    info!(
                        "[faces_diag][quic_client][preconnect] ok reason={} conn_id={} elapsed_ms={}",
                        reason,
                        conn_id,
                        started_at.elapsed().as_millis()
                    );
                    Ok(())
                }
                Err(()) => {
                    warn!(
                        "[faces_diag][quic_client][preconnect] failed reason={} elapsed_ms={}",
                        reason,
                        started_at.elapsed().as_millis()
                    );
                    Err(())
                }
            }
        }

        fn ensure_connection_for_request(
            &self,
            kind: &str,
            path: &Option<String>,
            req_id: Uuid,
        ) -> Result<(), ()> {
            let started_at = Instant::now();
            let was_off = self.http3_client.is_off();
            info!(
                "[faces_diag][quic_client][request] begin kind={} path={} request_id={} connection_off={}",
                kind,
                Self::path_for_log(path),
                req_id,
                was_off
            );

            if was_off {
                let _connect_guard = self.connect_lock.lock().unwrap();
                if !self.http3_client.is_off() {
                    info!(
                        "[faces_diag][quic_client][request] connection_reuse_after_wait kind={} path={} request_id={} elapsed_ms={}",
                        kind,
                        Self::path_for_log(path),
                        req_id,
                        started_at.elapsed().as_millis()
                    );
                    return Ok(());
                }

                info!(
                    "[faces_diag][quic_client][request] connection_needed kind={} path={} request_id={} timeout_ms={}",
                    kind,
                    Self::path_for_log(path),
                    req_id,
                    Self::request_connect_timeout(kind, path).as_millis()
                );
                match self
                    .http3_client
                    .connect_with_timeout(Self::request_connect_timeout(kind, path))
                {
                    Ok((conn_id, waker)) => {
                        *self.waker.lock().unwrap() = Some(waker);
                        info!(
                            "[faces_diag][quic_client][request] connection_ready kind={} path={} request_id={} conn_id={} elapsed_ms={}",
                            kind,
                            Self::path_for_log(path),
                            req_id,
                            conn_id,
                            started_at.elapsed().as_millis()
                        );
                        Ok(())
                    }
                    Err(()) => {
                        warn!(
                            "[faces_diag][quic_client][request] connection_failed kind={} path={} request_id={} elapsed_ms={}",
                            kind,
                            Self::path_for_log(path),
                            req_id,
                            started_at.elapsed().as_millis()
                        );
                        Err(())
                    }
                }
            } else {
                info!(
                    "[faces_diag][quic_client][request] connection_reuse kind={} path={} request_id={} elapsed_ms={}",
                    kind,
                    Self::path_for_log(path),
                    req_id,
                    started_at.elapsed().as_millis()
                );
                Ok(())
            }
        }

        fn enqueue_headers_for_request(
            &self,
            kind: &str,
            path: &Option<String>,
            req_id: Uuid,
            http3_request: &[Http3RequestPrep],
        ) {
            for req in http3_request {
                if let Http3RequestPrep::Header(header_req) = req {
                    let enqueue_started_at = Instant::now();
                    let adjust_sending_duration = crossbeam::channel::bounded::<Instant>(1);
                    info!(
                        "[faces_diag][quic_client][request] header_enqueue kind={} path={} request_id={}",
                        kind,
                        Self::path_for_log(path),
                        req_id
                    );
                    if self
                        .request_head
                        .send_request((
                            Http3Request::Header(header_req.clone()),
                            adjust_sending_duration.0,
                        ))
                        .is_err()
                    {
                        warn!(
                            "[faces_diag][quic_client][request] header_enqueue_failed kind={} path={} request_id={}",
                            kind,
                            Self::path_for_log(path),
                            req_id
                        );
                        println!("Error sending header request");
                    } else {
                        info!(
                            "[faces_diag][quic_client][request] header_enqueued kind={} path={} request_id={} elapsed_ms={}",
                            kind,
                            Self::path_for_log(path),
                            req_id,
                            enqueue_started_at.elapsed().as_millis()
                        );
                        self.wake_client();
                    }
                }
            }
        }

        fn wait_stream_ids_for_request(
            kind: &str,
            path: &Option<String>,
            req_id: Uuid,
            http3_confirm: Option<Http3RequestConfirm>,
        ) -> Result<(u64, String), ()> {
            let wait_started_at = Instant::now();
            info!(
                "[faces_diag][quic_client][request] wait_stream_ids_start kind={} path={} request_id={}",
                kind,
                Self::path_for_log(path),
                req_id
            );
            let Some(http3_confirm) = http3_confirm else {
                warn!(
                    "[faces_diag][quic_client][request] wait_stream_ids_missing_confirm kind={} path={} request_id={}",
                    kind,
                    Self::path_for_log(path),
                    req_id
                );
                return Err(());
            };

            match http3_confirm.wait_stream_ids() {
                Ok(stream_ids) => {
                    info!(
                        "[faces_diag][quic_client][request] wait_stream_ids_ok kind={} path={} request_id={} stream_id={} conn_id={} elapsed_ms={}",
                        kind,
                        Self::path_for_log(path),
                        req_id,
                        stream_ids.0,
                        stream_ids.1.as_str(),
                        wait_started_at.elapsed().as_millis()
                    );
                    Ok(stream_ids)
                }
                Err(e) => {
                    warn!(
                        "[faces_diag][quic_client][request] wait_stream_ids_failed kind={} path={} request_id={} elapsed_ms={} error={:?}",
                        kind,
                        Self::path_for_log(path),
                        req_id,
                        wait_started_at.elapsed().as_millis(),
                        e
                    );
                    Err(())
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
            let req_id = http3_request_builder.req_uuid();
            my_log::debug("ici connexion ping");
            match http3_request_builder.build_down_stream(keep_alive) {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    self.ensure_connection_for_request("downstream", &path, req_id)?;
                    self.enqueue_headers_for_request("downstream", &path, req_id, &http3_request);
                    // Once the stream has been created, we receive it back from the client loop.
                    let stream_ids = Self::wait_stream_ids_for_request(
                        "downstream",
                        &path,
                        req_id,
                        http3_confirm,
                    )?;
                    let stream_id = stream_ids.0;

                    self.stream_id_counter
                        .store(stream_id, std::sync::atomic::Ordering::Relaxed);

                    http3_request_builder.set_long_connection_stream_id(stream_id);

                    for req in &http3_request {
                        match req {
                            Http3RequestPrep::Ping(duration) => {
                                let ping_control = PingEmitter::run(
                                    *duration,
                                    &self.request_head,
                                    stream_id,
                                    &self.waker,
                                    &self.thread_controller,
                                );
                                http3_request_builder.set_stream_ping_controller(ping_control);
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

                    let thread_controller = self.thread_controller.clone();
                    std::thread::spawn(move || {
                        let (partial_response, completed_channel, progress_channel) =
                            PartialResponse::new_streamable(
                                path.unwrap().as_str(),
                                event_subscriber,
                                StreamSub::Downstream(stream_cb_syncable),
                                &stream_ids,
                                req_id,
                            );

                        let peer_response = WaitPeerResponse::new(
                            &stream_ids,
                            completed_channel,
                            progress_channel,
                            &thread_controller,
                        );
                        if let Err(e) = response_sender.send(peer_response) {
                            println!("Error: sending back WaitPeerResponse failed stream_id[{:?}] [{:?}]",stream_ids,e);
                        }

                        //send partial response to the reponse manager
                        if let Err(e) = response_manager_submission.submit(partial_response) {
                            println!("Error: failed to submit Partial response for stream_id[{:?}]   [{:?}]", stream_ids,e );
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
            let req_id = http3_request_builder.req_uuid();
            match http3_request_builder.build() {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    self.ensure_connection_for_request("builder", &path, req_id)?;
                    self.enqueue_headers_for_request("builder", &path, req_id, &http3_request);
                    let stream_ids =
                        Self::wait_stream_ids_for_request("builder", &path, req_id, http3_confirm)?;
                    let stream_id = stream_ids.0;
                    self.stream_id_counter
                        .store(stream_id, std::sync::atomic::Ordering::Relaxed);

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

                    let thread_controller = self.thread_controller.clone();
                    std::thread::spawn(move || {
                        let (partial_response, completed_channel, progress_channel) =
                            PartialResponse::new(
                                path.unwrap().as_str(),
                                event_subscriber,
                                &stream_ids,
                                req_id,
                            );

                        let peer_response = WaitPeerResponse::new(
                            &stream_ids,
                            completed_channel,
                            progress_channel,
                            &thread_controller,
                        );
                        if let Err(e) = response_sender.send(peer_response) {
                            println!("Error: sending back WaitPeerResponse failed stream_id[{:?}] [{:?}]",stream_ids,e);
                        }

                        //send partial response to the reponse manager
                        if let Err(e) = response_manager_submission.submit(partial_response) {
                            println!("Error: failed to submit Partial response for stream_id[{:?}]   [{:?}]", stream_ids,e );
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

            let request_id = http3_request_builder.req_uuid();
            let path = http3_request_builder.get_path();
            match http3_request_builder.build() {
                Ok((http3_request, event_subscriber, http3_confirm)) => {
                    self.ensure_connection_for_request("direct", &path, request_id)?;
                    self.enqueue_headers_for_request("direct", &path, request_id, &http3_request);
                    let stream_ids = Self::wait_stream_ids_for_request(
                        "direct",
                        &path,
                        request_id,
                        http3_confirm,
                    )?;
                    let stream_id = stream_ids.0;
                    self.stream_id_counter
                        .store(stream_id, std::sync::atomic::Ordering::Relaxed);

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

                    let thread_controller = self.thread_controller.clone();
                    std::thread::spawn(move || {
                        let (partial_response, completed_channel, progress_channel) =
                            PartialResponse::new(
                                path.unwrap().as_str(),
                                event_subscriber,
                                &stream_ids,
                                request_id,
                            );

                        let peer_response = WaitPeerResponse::new(
                            &stream_ids,
                            completed_channel,
                            progress_channel,
                            &thread_controller,
                        );
                        if let Err(e) = response_sender.send(peer_response) {
                            println!("Error: sending back WaitPeerResponse failed stream_id[{:?}] [{:?}]",stream_ids,e);
                        }

                        //send partial response to the reponse manager
                        if let Err(e) = response_manager_submission.submit(partial_response) {
                            println!("Error: failed to submit Partial response for stream_id[{:?}]   [{:?}]", stream_ids,e );
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
