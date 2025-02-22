mod quiche_http3_client;
pub use http3_client::Http3Client;

mod http3_client {
    use std::{
        clone,
        sync::{Arc, Mutex},
    };

    use crate::{
        client_config::{self, ClientConfig},
        client_manager::{BodyQueue, RequestQueue, ResponseHead},
    };

    use super::*;

    pub struct Http3Client {
        client_config: Arc<ClientConfig>,
        request_queue: RequestQueue,
        response_head: ResponseHead,
        body_queue: BodyQueue,
        connexion_opened: Arc<Mutex<bool>>,
    }

    impl Http3Client {
        pub fn new(
            client_configuration: ClientConfig,
            request_queue: RequestQueue,
            response_head: ResponseHead,
            body_queue: BodyQueue,
        ) -> Self {
            Self {
                client_config: Arc::new(client_configuration),
                request_queue,
                body_queue,
                response_head,
                connexion_opened: Arc::new(Mutex::new(false)),
            }
        }

        ///
        ///Check if a connexion is already started with the requested server
        ///
        pub fn is_off(&self) -> bool {
            *self.connexion_opened.lock().unwrap()
        }
        ///
        ///Block and wait for the connexion making.
        ///return the connexion id String.
        ///
        ///
        pub fn connect(&self) -> Result<String, ()> {
            if let Ok(conn_id) = self.run() {
                *self.connexion_opened.lock().unwrap() = true;
                println!("Connexion [{:?}] is opened", conn_id);
                Ok(conn_id)
            } else {
                Err(())
            }
        }

        ///
        ///Run the http3 client in a separate Os thread with the client_config.
        ///
        pub fn run(&self) -> Result<String, crossbeam::channel::RecvError> {
            let configuration_clone = self.client_config.clone();
            let req_queue = self.request_queue.clone();
            let resp_head = self.response_head.clone();
            let body_queue = self.body_queue.clone();
            let connexion_opened = self.connexion_opened.clone();
            let confirm_connexion_chan = crossbeam::channel::bounded::<String>(1);
            let confirmation_sender = confirm_connexion_chan.0.clone();

            std::thread::spawn(move || {
                if let Ok(finished) = quiche_http3_client::run(
                    configuration_clone,
                    req_queue,
                    resp_head,
                    body_queue,
                    confirmation_sender,
                ) {
                    println!("Closing connexion [{:?}]", finished);
                    *connexion_opened.lock().unwrap() = false;
                };
            });
            confirm_connexion_chan.1.recv()
        }
    }
}
