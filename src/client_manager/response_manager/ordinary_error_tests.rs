use super::{
    response_manager_worker, response_mngr::PartialResponseReceiver, CompletedResponse,
    DownloadProgressStatus, Http3Response, PartialResponse, ResponseChannel, UploadProgressStatus,
};
use crate::{
    client_manager::request_manager::RequestEvent, thread_controller::ThreadController,
    RequestEventListener,
};
use crossbeam::channel::{bounded, Receiver};
use quiche::h3::Header;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;
const WAIT: Duration = Duration::from_secs(1);
const CONNECTION: &str = "ordinary-error-test";
fn partial(id: u64) -> (PartialResponse, Receiver<CompletedResponse>) {
    let (response, completed, _) = PartialResponse::new(
        "/ordinary",
        vec![],
        &(id, CONNECTION.into()),
        Uuid::new_v4(),
    );
    (response, completed)
}
fn headers(id: u64, status: &[u8], length: Option<&[u8]>, end: bool) -> Http3Response {
    let mut headers = vec![Header::new(b":status", status)];
    if let Some(length) = length {
        headers.push(Header::new(b"content-length", length));
    }
    Http3Response::new_header(id, CONNECTION.into(), headers, end)
}
fn body(id: u64, bytes: &[u8], end: bool) -> Http3Response {
    Http3Response::new_body_data(id, CONNECTION.into(), bytes, end)
}
#[test]
fn ordinary_errors_complete_once_with_missing_zero_invalid_or_positive_length() {
    for status in [b"401", b"403", b"404", b"503", b"302"] {
        for length in [None, Some(b"0".as_slice()), Some(b"invalid"), Some(b"13")] {
            let (mut partial, completed) = partial(0);
            assert!(!partial.extend_data(headers(0, status, length, false)));
            assert!(!partial.extend_data(body(0, b"token_", false)));
            assert!(!partial.extend_data(body(0, b"expired", false)));
            assert!(completed.try_recv().is_err());
            assert!(partial.extend_data(body(0, b"", true)));
            assert!(partial.extend_data(body(0, b"late", true)));
            assert!(partial.extend_data(headers(0, status, length, true)));
            let response = completed.recv_timeout(WAIT).unwrap();
            assert_eq!(response.status_code().unwrap(), status);
            assert_eq!(response.as_data(), b"token_expired");
            assert!(completed.try_recv().is_err());
        }
    }
}
#[test]
fn ordinary_empty_errors_accept_both_header_fin_and_separate_fin() {
    for length in [None, Some(b"0".as_slice()), Some(b"invalid")] {
        for header_fin in [false, true] {
            let (mut partial, completed) = partial(0);
            assert_eq!(
                partial.extend_data(headers(0, b"401", length, header_fin)),
                header_fin
            );
            assert!(partial.extend_data(body(0, b"", true)));
            let response = completed.recv_timeout(WAIT).unwrap();
            assert_eq!(response.status_code().unwrap(), b"401");
            assert!(response.as_data().is_empty());
            assert!(completed.try_recv().is_err());
        }
    }
}
#[test]
fn ordinary_error_fin_can_arrive_on_the_last_fragment_after_100() {
    let (mut partial, completed) = partial(0);
    assert!(!partial.extend_data(headers(0, b"100", None, false)));
    assert!(!partial.extend_data(headers(0, b"401", None, false)));
    for byte in b"token_expire" {
        assert!(!partial.extend_data(body(0, &[*byte], false)));
    }
    assert!(partial.extend_data(body(0, b"d", true)));
    let response = completed.recv_timeout(WAIT).unwrap();
    assert_eq!(response.as_data(), b"token_expired");
    assert_eq!(response.status_code().unwrap(), b"401");
}
struct Progress(Arc<Mutex<Vec<(&'static str, f32)>>>);
impl RequestEventListener for Progress {
    fn on_download_progress(
        &self,
        progress: DownloadProgressStatus,
    ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
        self.0
            .lock()
            .unwrap()
            .push(("download", progress.progress()));
        Ok(())
    }
    fn on_upload_progress(
        &self,
        progress: UploadProgressStatus,
    ) -> Result<(), crossbeam::channel::SendError<RequestEvent>> {
        self.0.lock().unwrap().push(("upload", progress.progress()));
        Ok(())
    }
}
#[test]
fn known_length_success_and_error_keep_their_progress_events() {
    for status in [b"200", b"401"] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (mut partial, completed, _) = PartialResponse::new(
            "/ordinary",
            vec![Arc::new(Progress(events.clone()))],
            &(0, CONNECTION.into()),
            Uuid::new_v4(),
        );
        assert!(!partial.extend_data(headers(0, status, Some(b"4"), false)));
        assert!(!partial.extend_data(body(0, b"ab", false)));
        assert!(partial.extend_data(body(0, b"cd", true)));
        assert_eq!(completed.recv_timeout(WAIT).unwrap().as_data(), b"abcd");
        assert_eq!(
            *events.lock().unwrap(),
            vec![("download", 0.5), ("download", 1.0), ("upload", 1.0)]
        );
    }
}
#[test]
fn missing_error_length_does_not_invent_progress_totals() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let (mut partial, completed, _) = PartialResponse::new(
        "/ordinary",
        vec![Arc::new(Progress(events.clone()))],
        &(0, CONNECTION.into()),
        Uuid::new_v4(),
    );
    assert!(!partial.extend_data(headers(0, b"401", None, false)));
    assert!(partial.extend_data(body(0, b"token_expired", true)));
    assert_eq!(
        completed.recv_timeout(WAIT).unwrap().as_data(),
        b"token_expired"
    );
    assert!(events.lock().unwrap().is_empty());
}
#[test]
fn worker_continues_on_the_same_connection_after_an_error_without_length() {
    let controller = ThreadController::new("ordinary-errors");
    let channel = ResponseChannel::new(&controller);
    let head = channel.get_head();
    let (error, error_completed) = partial(0);
    let (success, success_completed) = partial(4);
    let table = Arc::new(Mutex::new(HashMap::from([
        ((0, CONNECTION.into()), error),
        ((4, CONNECTION.into()), success),
    ])));
    let (sender, receiver) = bounded(1);
    response_manager_worker::run(
        channel.get_queue(),
        PartialResponseReceiver::new(receiver, &controller),
        &controller,
        &Arc::new(Mutex::new(HashMap::new())),
        table.clone(),
        Arc::new(AtomicU64::new(0)),
    );
    for response in [
        headers(0, b"401", None, false),
        body(0, b"token_expired", false),
        body(0, b"", true),
        headers(4, b"200", Some(b"2"), false),
        body(4, b"ok", true),
    ] {
        head.send_response(response).unwrap();
    }
    assert_eq!(
        error_completed
            .recv_timeout(WAIT)
            .unwrap()
            .status_code()
            .unwrap(),
        b"401"
    );
    assert_eq!(
        success_completed.recv_timeout(WAIT).unwrap().as_data(),
        b"ok"
    );
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(table) = table.try_lock() {
            if table.is_empty() {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "response worker retained terminal entries"
        );
        std::thread::yield_now();
    }
    drop(head);
    drop(channel);
    drop(sender);
}
