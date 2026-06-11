mod quiche_http3_client;
pub use http3_client::Http3Client;

mod http3_client {
    use std::{
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex,
        },
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
    const CONNECT_CONFIRMATION_POLL_INTERVAL: Duration = Duration::from_millis(50);

    pub struct Http3Client {
        client_config: Arc<ClientConfig>,
        request_queue: RequestQueue,
        response_head: ResponseHead,
        body_queue: BodyQueue,
        connexion_opened: Arc<Mutex<bool>>,
        thread_controller: ThreadController,
        connect_generation: Arc<AtomicU64>,
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
                connect_generation: Arc::new(AtomicU64::new(0)),
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

        pub fn invalidate_pending_connects(&self, reason: &str) {
            let previous_generation = self.connect_generation.fetch_add(1, Ordering::Relaxed);
            let generation = previous_generation + 1;
            log::warn!(
                "[faces_diag][quic_client][connect] invalidate_pending reason={} previous_generation={} generation={}",
                reason,
                previous_generation,
                generation
            );
            *self.connexion_opened.lock().unwrap() = false;
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
            let generation = self.connect_generation.fetch_add(1, Ordering::Relaxed) + 1;
            log::info!(
                "[faces_diag][quic_client][connect] start peer={:?} local={:?} was_off={} timeout_ms={} generation={}",
                self.client_config.peer_address(),
                self.client_config.local_address(),
                was_off,
                confirmation_timeout.as_millis(),
                generation
            );
            match self.run_at_generation(confirmation_timeout, generation) {
                Ok((conn_id, waker)) => {
                    let current_generation = self.connect_generation.load(Ordering::Relaxed);
                    if current_generation != generation {
                        log::warn!(
                            "[faces_diag][quic_client][connect] stale_confirmation_rejected conn_id={} generation={} current_generation={} elapsed_ms={}",
                            conn_id,
                            generation,
                            current_generation,
                            started_at.elapsed().as_millis()
                        );
                        return Err(());
                    }
                    *self.connexion_opened.lock().unwrap() = true;
                    log::info!(
                        "[faces_diag][quic_client][connect] ok conn_id={} elapsed_ms={} generation={}",
                        conn_id,
                        started_at.elapsed().as_millis(),
                        generation
                    );
                    Ok((conn_id, waker))
                }
                Err(_) => {
                    log::warn!(
                        "[faces_diag][quic_client][connect] failed elapsed_ms={} generation={}",
                        started_at.elapsed().as_millis(),
                        generation
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
            let generation = self.connect_generation.fetch_add(1, Ordering::Relaxed) + 1;
            self.run_at_generation(confirmation_timeout, generation)
        }

        fn run_at_generation(
            &self,
            confirmation_timeout: Duration,
            generation: u64,
        ) -> Result<(String, Waker), crossbeam::channel::RecvError> {
            let configuration_clone = self.client_config.clone();
            let req_queue = self.request_queue.clone();
            let resp_head = self.response_head.clone();
            let body_queue = self.body_queue.clone();
            let connexion_opened = self.connexion_opened.clone();
            let connect_generation = self.connect_generation.clone();
            let confirm_connexion_chan = crossbeam::channel::bounded::<(String, Waker)>(1);
            let confirmation_started_at = Instant::now();
            let confirmation_sender = confirm_connexion_chan.0.clone();

            log::info!(
                "[faces_diag][quic_client][connect] run_spawn peer={:?} local={:?} generation={}",
                configuration_clone.peer_address(),
                configuration_clone.local_address(),
                generation
            );

            let thread_controller = self.thread_controller.clone();
            std::thread::spawn(move || {
                let loop_started_at = Instant::now();
                log::info!(
                    "[faces_diag][quic_client][connect] loop_thread_start generation={}",
                    generation
                );
                let result = quiche_http3_client::run(
                    configuration_clone,
                    req_queue,
                    resp_head,
                    body_queue,
                    confirmation_sender,
                    &thread_controller,
                    connect_generation.clone(),
                    generation,
                );
                let current_generation = connect_generation.load(Ordering::Relaxed);
                if current_generation == generation {
                    *connexion_opened.lock().unwrap() = false;
                } else {
                    log::info!(
                        "[faces_diag][quic_client][connect] loop_thread_stale_end generation={} current_generation={} elapsed_ms={}",
                        generation,
                        current_generation,
                        loop_started_at.elapsed().as_millis()
                    );
                }
                match &result {
                    Ok(conn_id) => log::info!(
                        "[faces_diag][quic_client][connect] loop_thread_end result=ok conn_id={} elapsed_ms={} generation={}",
                        conn_id,
                        loop_started_at.elapsed().as_millis(),
                        generation
                    ),
                    Err(_) => log::warn!(
                        "[faces_diag][quic_client][connect] loop_thread_end result=err elapsed_ms={} generation={}",
                        loop_started_at.elapsed().as_millis(),
                        generation
                    ),
                }
                if let Err(e) = result {
                    log::info!(
                        "[faces_diag][quic_client] client loop ended with error [{:?}]",
                        e
                    );
                };
            });

            loop {
                let current_generation = self.connect_generation.load(Ordering::Relaxed);
                if current_generation != generation {
                    log::warn!(
                        "[faces_diag][quic_client][connect] confirmation_cancelled generation={} current_generation={} elapsed_ms={}",
                        generation,
                        current_generation,
                        confirmation_started_at.elapsed().as_millis()
                    );
                    return Err(crossbeam::channel::RecvError);
                }

                let elapsed = confirmation_started_at.elapsed();
                if elapsed >= confirmation_timeout {
                    log::warn!(
                        "[faces_diag][quic_client][connect] confirmation_timeout timeout_ms={} elapsed_ms={} generation={}",
                        confirmation_timeout.as_millis(),
                        elapsed.as_millis(),
                        generation
                    );
                    log::info!(
                        "[faces_diag][quic_client] connect confirmation timeout after {}ms",
                        confirmation_timeout.as_millis()
                    );
                    return Err(crossbeam::channel::RecvError);
                }

                let remaining = confirmation_timeout - elapsed;
                let wait_for = if remaining < CONNECT_CONFIRMATION_POLL_INTERVAL {
                    remaining
                } else {
                    CONNECT_CONFIRMATION_POLL_INTERVAL
                };

                match confirm_connexion_chan.1.recv_timeout(wait_for) {
                    Ok((conn_id, waker)) => {
                        log::info!(
                            "[faces_diag][quic_client][connect] confirmation_received conn_id={} elapsed_ms={} generation={}",
                            conn_id,
                            confirmation_started_at.elapsed().as_millis(),
                            generation
                        );
                        return Ok((conn_id, waker));
                    }
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                        log::warn!(
                            "[faces_diag][quic_client][connect] confirmation_disconnected elapsed_ms={} generation={}",
                            confirmation_started_at.elapsed().as_millis(),
                            generation
                        );
                        return Err(crossbeam::channel::RecvError);
                    }
                }
            }
        }
    }
}
