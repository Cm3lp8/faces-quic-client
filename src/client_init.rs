mod quiche_http3_client;
pub use http3_client::Http3Client;

mod http3_client {
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use mio::Waker;

    use crate::{
        client_config::ClientConfig,
        client_manager::{BodyQueue, RequestQueue, ResponseHead},
        thread_controller::{self, ThreadController},
    };

    use super::*;

    const DEFAULT_CONNECT_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(10);

    pub struct Http3Client {
        client_config: Arc<ClientConfig>,
        request_queue: RequestQueue,
        response_head: ResponseHead,
        body_queue: BodyQueue,
        connexion_opened: Arc<Mutex<bool>>,
        thread_controller: ThreadController,
    }

    impl Http3Client {
        pub fn new(
            client_configuration: ClientConfig,
            request_queue: RequestQueue,
            response_head: ResponseHead,
            body_queue: BodyQueue,
            thread_controller: &ThreadController,
        ) -> Self {
            Self {
                client_config: Arc::new(client_configuration),
                request_queue,
                body_queue,
                response_head,
                connexion_opened: Arc::new(Mutex::new(false)),
                thread_controller: thread_controller.clone(),
            }
        }

        ///
        ///Check if a connexion is already started with the requested server
        ///
        pub fn is_off(&self) -> bool {
            !*self.connexion_opened.lock().unwrap()
        }
        pub fn mark_disconnected(&self) {
            log::info!("[faces_diag][quic_client][connect] mark_disconnected");
            *self.connexion_opened.lock().unwrap() = false;
        }
        pub fn drain_stale_requests(&self, reason: &str) -> usize {
            self.request_queue.drain_stale(reason)
        }
        ///
        ///Block and wait for the connexion making.
        ///return the connexion id String.
        ///
        ///
        pub fn connect(&self) -> Result<(String, Waker), ()> {
            self.connect_with_timeout(DEFAULT_CONNECT_CONFIRMATION_TIMEOUT)
        }

        pub fn connect_with_timeout(
            &self,
            confirmation_timeout: Duration,
        ) -> Result<(String, Waker), ()> {
            let started_at = Instant::now();
            let was_off = self.is_off();
            log::info!(
                "[faces_diag][quic_client][connect] start peer={:?} local={:?} was_off={} timeout_ms={}",
                self.client_config.peer_address(),
                self.client_config.local_address(),
                was_off,
                confirmation_timeout.as_millis()
            );
            match self.run(confirmation_timeout) {
                Ok((conn_id, waker)) => {
                    *self.connexion_opened.lock().unwrap() = true;
                    log::info!(
                        "[faces_diag][quic_client][connect] ok conn_id={} elapsed_ms={}",
                        conn_id,
                        started_at.elapsed().as_millis()
                    );
                    Ok((conn_id, waker))
                }
                Err(_) => {
                    log::warn!(
                        "[faces_diag][quic_client][connect] failed elapsed_ms={}",
                        started_at.elapsed().as_millis()
                    );
                    Err(())
                }
            }
        }

        ///
        ///Run the http3 client in a separate Os thread with the client_config.
        ///
        pub fn run(
            &self,
            confirmation_timeout: Duration,
        ) -> Result<(String, Waker), crossbeam::channel::RecvError> {
            let configuration_clone = self.client_config.clone();
            let req_queue = self.request_queue.clone();
            let resp_head = self.response_head.clone();
            let body_queue = self.body_queue.clone();
            let connexion_opened = self.connexion_opened.clone();
            let confirm_connexion_chan = crossbeam::channel::bounded::<(String, Waker)>(1);
            let confirmation_started_at = Instant::now();
            let confirmation_sender = confirm_connexion_chan.0.clone();

            log::info!(
                "[faces_diag][quic_client][connect] run_spawn peer={:?} local={:?}",
                configuration_clone.peer_address(),
                configuration_clone.local_address()
            );

            let thread_controller = self.thread_controller.clone();
            std::thread::spawn(move || {
                let loop_started_at = Instant::now();
                log::info!("[faces_diag][quic_client][connect] loop_thread_start");
                let result = quiche_http3_client::run(
                    configuration_clone,
                    req_queue,
                    resp_head,
                    body_queue,
                    confirmation_sender,
                    &thread_controller,
                );
                *connexion_opened.lock().unwrap() = false;
                match &result {
                    Ok(conn_id) => log::info!(
                        "[faces_diag][quic_client][connect] loop_thread_end result=ok conn_id={} elapsed_ms={}",
                        conn_id,
                        loop_started_at.elapsed().as_millis()
                    ),
                    Err(_) => log::warn!(
                        "[faces_diag][quic_client][connect] loop_thread_end result=err elapsed_ms={}",
                        loop_started_at.elapsed().as_millis()
                    ),
                }
                if let Err(e) = result {
                    log::info!(
                        "[faces_diag][quic_client] client loop ended with error [{:?}]",
                        e
                    );
                };
            });
            match confirm_connexion_chan.1.recv_timeout(confirmation_timeout) {
                Ok((conn_id, waker)) => {
                    log::info!(
                        "[faces_diag][quic_client][connect] confirmation_received conn_id={} elapsed_ms={}",
                        conn_id,
                        confirmation_started_at.elapsed().as_millis()
                    );
                    Ok((conn_id, waker))
                }
                Err(_) => {
                    log::warn!(
                        "[faces_diag][quic_client][connect] confirmation_timeout timeout_ms={} elapsed_ms={}",
                        confirmation_timeout.as_millis(),
                        confirmation_started_at.elapsed().as_millis()
                    );
                    log::info!(
                        "[faces_diag][quic_client] connect confirmation timeout after {}ms",
                        confirmation_timeout.as_millis()
                    );
                    Err(crossbeam::channel::RecvError)
                }
            }
        }
    }
}
