pub use event_listener_implementations::ProgressTracker;
pub use event_listener_implementations::RequestEvent;
pub use request_event_listener_trait::RequestEventListener;
mod request_event_listener_trait {
    use crate::client_manager::{response_manager::DownloadProgressStatus, UploadProgressStatus};

    use super::RequestEvent;

    pub trait RequestEventListener {
        fn on_upload_progress(
            &self,
            upload_progress: UploadProgressStatus,
        ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
            Ok(())
        }
        fn on_download_progress(
            &self,
            download_progress: DownloadProgressStatus,
        ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
            Ok(())
        }
        fn on_stream_info(&self) {}
        fn on_data(&self) {}
    }
}

mod event_listener_implementations {
    use std::{path::PathBuf, sync::Arc};

    use uuid::Uuid;

    use crate::client_manager::{response_manager::DownloadProgressStatus, UploadProgressStatus};

    use super::RequestEventListener;
    pub enum RequestEvent {
        UploadProgress(UploadProgressStatus),
        DownloadProgress(DownloadProgressStatus),
        ConnexionClosed(Vec<u8>),
    }

    pub trait EventAccess {
        fn request_uuid(&self) -> Uuid;
        fn request_path(&self) -> String;
    }

    pub struct ProgressTracker {
        sender: crossbeam::channel::Sender<RequestEvent>,
        receiver: crossbeam::channel::Receiver<RequestEvent>,
    }

    impl ProgressTracker {
        pub fn new() -> Arc<Self> {
            let chann = crossbeam::channel::unbounded::<RequestEvent>();
            let progress_tracker = Self {
                sender: chann.0,
                receiver: chann.1,
            };
            Arc::new(progress_tracker)
        }
        pub fn run(&self, cb: impl Fn(RequestEvent) + Send + Sync + 'static) {
            let recv = self.receiver.clone();

            std::thread::spawn(move || {
                while let Ok(event) = recv.recv() {
                    cb(event)
                }
            });
        }
    }

    impl RequestEventListener for ProgressTracker {
        fn on_upload_progress(
            &self,
            upload_progress: UploadProgressStatus,
        ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
            self.sender
                .send(RequestEvent::UploadProgress(upload_progress))
        }
        fn on_download_progress(
            &self,
            download_progress: DownloadProgressStatus,
        ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
            self.sender
                .send(RequestEvent::DownloadProgress(download_progress))
        }
    }

    impl EventAccess for UploadProgressStatus {
        fn request_uuid(&self) -> Uuid {
            self.uuid()
        }
        fn request_path(&self) -> String {
            self.req_path()
        }
    }
}
