pub use body_builder::Http3Body;
pub use body_queue_builder::{BodyChannel, BodyHead, BodyQueue};
mod body_queue_builder {
    use super::*;

    pub struct BodyChannel {
        channel: (
            crossbeam::channel::Sender<Http3Body>,
            crossbeam::channel::Receiver<Http3Body>,
        ),
    }
    pub struct BodyHead {
        head: crossbeam::channel::Sender<Http3Body>,
    }

    pub struct BodyQueue {
        queue: crossbeam::channel::Receiver<Http3Body>,
    }

    impl Clone for BodyHead {
        fn clone(&self) -> Self {
            Self {
                head: self.head.clone(),
            }
        }
    }

    impl BodyChannel {
        pub fn new() -> Self {
            Self {
                channel: crossbeam::channel::unbounded(),
            }
        }
        pub fn get_head(&self) -> BodyHead {
            BodyHead {
                head: self.channel.0.clone(),
            }
        }
        pub fn get_queue(&self) -> BodyQueue {
            BodyQueue {
                queue: self.channel.1.clone(),
            }
        }
    }
}

mod body_builder {
    use super::*;

    pub struct Http3Body {
        stream_id: u64,
        data: Vec<u8>,
        fin: bool,
    }
}
