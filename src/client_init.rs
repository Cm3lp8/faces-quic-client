mod quiche_http3_client;

mod http3_client {
    use std::sync::Arc;

    use crate::client_config::{self, ClientConfig};

    use super::*;

    pub struct Http3Client {
        client_config: Arc<ClientConfig>,
    }

    impl Http3Client {
        pub fn new(client_configuration: ClientConfig) -> Self {
            Self {
                client_config: Arc::new(client_configuration),
            }
        }

        ///
        ///Run the http3 client in a separate Os thread with the client_config.
        ///
        pub fn run(&self) {
            let configuration_clone = self.client_config.clone();

            std::thread::spawn(move || {});
        }
    }
}
