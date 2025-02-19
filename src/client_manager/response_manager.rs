pub use queue_builder::{ResponseChannel, ResponseHead, ResponseQueue};
pub use response_builder::Http3Response;

mod response_mngr {
    use super::*;

    pub struct ResponseManager {
        response_queue: ResponseQueue,
    }
    impl ResponseManager {
        pub fn run(&self) {
            u
        }
    }
}
mod queue_builder {
    use super::*;

    pub struct ResponseChannel {
        channel: (
            crossbeam::channel::Sender<Http3Response>,
            crossbeam::channel::Receiver<Http3Response>,
        ),
    }
    pub struct ResponseHead {
        head: crossbeam::channel::Sender<Http3Response>,
    }

    pub struct ResponseQueue {
        queue: crossbeam::channel::Receiver<Http3Response>,
    }

    impl Clone for ResponseQueue {
        fn clone(&self) -> Self {
            Self {
                queue: self.queue.clone(),
            }
        }
    }

    impl ResponseChannel {
        pub fn new() -> Self {
            Self {
                channel: crossbeam::channel::unbounded(),
            }
        }
        pub fn get_head(&self) -> ResponseHead {
            ResponseHead {
                head: self.channel.0.clone(),
            }
        }
        pub fn get_queue(&self) -> ResponseQueue {
            ResponseQueue {
                queue: self.channel.1.clone(),
            }
        }
    }
}

mod response_builder {
    use super::*;

    pub struct Http3Response;
}
