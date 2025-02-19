pub use client_configuration::ClientConfig;
pub use connexion_info::ConnexionInfos;
mod client_configuration {
    use std::net::SocketAddr;

    use mio::net::UdpSocket;

    use self::connexion_info::ConnexionInfos;

    use super::*;

    ///
    ///Access connexion_infos with fn connexion_infos() -> &ConnexionInfos
    ///
    pub struct ClientConfig {
        connexion_info: ConnexionInfos,
    }

    impl ClientConfig {
        ///
        ///Init ClientConfig which give access to ConnexionInfos >> fn connexion_infos()
        ///
        pub fn new() -> ClientConfig {
            Self {
                connexion_info: ConnexionInfos::new(),
            }
        }
        pub fn local_address(&self) -> Option<SocketAddr> {
            self.connexion_info.get_local_socket_address()
        }
        pub fn peer_address(&self) -> Option<SocketAddr> {
            self.connexion_info.get_peer_socket_address()
        }
        pub fn connexion_infos(&self) -> &ConnexionInfos {
            &self.connexion_info
        }
    }
}

mod connexion_info {
    use std::{
        net::SocketAddr,
        sync::{Arc, Mutex},
    };

    use mio::net::UdpSocket;

    use super::*;

    ///
    ///Mutable state to keep track of sockets addresses.
    ///
    pub struct ConnexionInfos {
        inner: Arc<Mutex<ConnexionInfosInner>>,
    }

    impl Clone for ConnexionInfos {
        fn clone(&self) -> Self {
            Self {
                inner: self.inner.clone(),
            }
        }
    }

    impl ConnexionInfos {
        pub fn new() -> Self {
            Self {
                inner: Arc::new(Mutex::new(ConnexionInfosInner::new())),
            }
        }

        pub fn is_builded(&self) -> bool {
            let guard = &*self.inner.lock().unwrap();

            match guard {
                ConnexionInfosInner::SetUp(_) => false,
                ConnexionInfosInner::Builded(_) => true,
            }
        }

        pub fn get_local_socket_address(&self) -> Option<SocketAddr> {
            let guard = &*self.inner.lock().unwrap();

            if let ConnexionInfosInner::Builded(connexion_info) = guard {
                Some(connexion_info.local_socket)
            } else {
                None
            }
        }
        pub fn get_peer_socket_address(&self) -> Option<SocketAddr> {
            let guard = &*self.inner.lock().unwrap();

            if let ConnexionInfosInner::Builded(connexion_info) = guard {
                Some(connexion_info.distant_socket)
            } else {
                None
            }
        }

        ///
        ///If a new peer connexion is needed, call new_connexion_setup to reset connexion_infos
        ///
        ///
        pub fn new_connexion_setup(&self) -> &Self {
            let guard = &mut *self.inner.lock().unwrap();

            *guard = ConnexionInfosInner::new();
            self
        }

        pub fn set_local_address(&self, local_socket_address: &str) -> &Self {
            let guard = &mut *self.inner.lock().unwrap();

            match guard {
                ConnexionInfosInner::SetUp(conn_infos) => {
                    conn_infos.local_socket = Some(local_socket_address.parse().unwrap())
                }
                ConnexionInfosInner::Builded(_) => {
                    println!("already builded ! ")
                }
            }

            self
        }
        pub fn set_peer_address(&self, peer_socket_address: &str) -> &Self {
            let guard = &mut *self.inner.lock().unwrap();

            match guard {
                ConnexionInfosInner::SetUp(conn_infos) => {
                    conn_infos.distant_socket = Some(peer_socket_address.parse().unwrap())
                }
                ConnexionInfosInner::Builded(_) => {
                    println!("already builded ! ")
                }
            }

            self
        }

        /// Build connexion infos in place
        pub fn build_connexion_infos(&self) {
            let guard = &mut *self.inner.lock().unwrap();

            match guard {
                ConnexionInfosInner::Builded(_) => {
                    println!("Connexion infos already builded")
                }
                ConnexionInfosInner::SetUp(conn_info) => {
                    if conn_info.local_socket.is_none() || conn_info.distant_socket.is_none() {
                        println!("connexion infos incomplete local socket infos [{:?}]  distant_socket infos [{:?}]", conn_info.local_socket, conn_info.distant_socket);
                        return;
                    }

                    let build = ConnexionInfoBuilded {
                        distant_socket: conn_info.distant_socket.take().unwrap(),
                        local_socket: conn_info.local_socket.take().unwrap(),
                    };

                    *guard = ConnexionInfosInner::Builded(build);
                }
            }
            ()
        }
    }

    ///
    ///ConnectionInfos builder
    ///
    ///
    enum ConnexionInfosInner {
        SetUp(ConnexionInfosSetup),
        Builded(ConnexionInfoBuilded),
    }

    impl ConnexionInfosInner {
        pub fn new() -> ConnexionInfosInner {
            Self::SetUp(ConnexionInfosSetup::new())
        }
    }

    struct ConnexionInfosSetup {
        distant_socket: Option<SocketAddr>,
        local_socket: Option<SocketAddr>,
    }

    impl ConnexionInfosSetup {
        pub fn new() -> Self {
            Self {
                distant_socket: None,
                local_socket: None,
            }
        }
    }

    struct ConnexionInfoBuilded {
        distant_socket: SocketAddr,
        local_socket: SocketAddr,
    }
}

mod test_client_config {

    use self::client_configuration::ClientConfig;

    use super::*;

    #[test]
    fn client_config_test() {
        let client_configuration = ClientConfig::new();

        assert!(!client_configuration.connexion_infos().is_builded());

        let local = "0.0.0.0:0";
        let distant = "127.0.0.1:3000";

        client_configuration
            .connexion_infos()
            .set_local_address(local)
            .set_peer_address(distant)
            .build_connexion_infos();

        assert!(
            client_configuration
                .connexion_infos()
                .get_peer_socket_address()
                .is_some()
                && client_configuration
                    .connexion_infos()
                    .get_local_socket_address()
                    .is_some()
        );
    }
}
