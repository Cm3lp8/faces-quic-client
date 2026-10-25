use super::{
    response_manager_worker, response_mngr::PartialResponseReceiver, CompletedResponse,
    Http3Response, PartialResponse, ResponseChannel,
};
use crate::{client_manager::persistant_stream::StreamSub, thread_controller::ThreadController};
use crossbeam::channel::{bounded, unbounded, Receiver};
use quiche::h3::Header;
use std::{
    collections::HashMap,
    sync::{atomic::AtomicU64, Arc, Mutex},
    time::{Duration, Instant},
};
use stream_framer::FrameWriter;
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(1);
const CONNECTION: &str = "stream-http-errors-test";

fn partial(
    stream_id: u64,
    streamable: bool,
) -> (
    PartialResponse,
    Receiver<CompletedResponse>,
    Receiver<Vec<u8>>,
) {
    let (frames, received_frames) = unbounded();
    let ids = (stream_id, CONNECTION.to_owned());
    let (partial, completed, _) = if streamable {
        PartialResponse::new_streamable(
            "/notification_bus",
            vec![],
            StreamSub::Downstream(Arc::new(move |event, _| {
                frames.send(event.body_as_slice().to_vec()).unwrap();
            })),
            &ids,
            Uuid::new_v4(),
        )
    } else {
        PartialResponse::new("/ordinary", vec![], &ids, Uuid::new_v4())
    };
    (partial, completed, received_frames)
}

fn headers(stream_id: u64, status: &[u8], length: Option<&[u8]>, end: bool) -> Http3Response {
    let mut headers = vec![Header::new(b":status", status)];
    if let Some(length) = length {
        headers.push(Header::new(b"content-length", length));
    }
    Http3Response::new_header(stream_id, CONNECTION.to_owned(), headers, end)
}

fn body(stream_id: u64, data: &[u8], end: bool) -> Http3Response {
    Http3Response::new_body_data(stream_id, CONNECTION.to_owned(), data, end)
}

fn assert_error_response(status: &[u8], chunks: &[&[u8]], length: Option<&[u8]>) {
    let (mut partial, completed, frames) = partial(0, true);
    assert!(!partial.extend_data(headers(0, status, length, false)));
    for chunk in chunks {
        assert!(!partial.extend_data(body(0, chunk, false)));
        assert!(
            completed.try_recv().is_err(),
            "response completed before FIN"
        );
    }
    assert!(
        partial.extend_data(body(0, b"", true)),
        "terminal error must retire its entry"
    );
    // Duplicate FIN and late packets cannot enqueue a second response or block
    // while the bounded completion channel still contains the first response.
    assert!(partial.extend_data(body(0, b"", true)));
    assert!(partial.extend_data(body(0, b"late-error-bytes", true)));
    assert!(partial.extend_data(headers(0, status, length, true)));
    let response = completed.recv_timeout(WAIT).unwrap();
    assert_eq!(response.status_code().unwrap(), status);
    assert_eq!(response.as_data(), chunks.concat());
    assert!(completed.try_recv().is_err());
    assert!(
        frames.try_recv().is_err(),
        "HTTP error was delivered as a notification"
    );
}

#[test]
fn expired_jwt_body_in_one_packet_completes_once_at_fin() {
    assert_error_response(b"401", &[b"token_expired"], Some(b"13"));
}

#[test]
fn fragmented_expired_jwt_body_completes_once_at_fin() {
    assert_error_response(b"401", &[b"token_", b"expired"], Some(b"13"));
}

#[test]
fn bytewise_expired_jwt_body_completes_without_notification_framing() {
    let chunks: Vec<&[u8]> = b"token_expired".chunks(1).collect();
    assert_error_response(b"401", &chunks, None);
}

#[test]
fn errors_preserve_status_and_body_without_framing() {
    for status in [b"403".as_slice(), b"404", b"503", b"302"] {
        assert_error_response(status, &[b"forbidden"], Some(b"9"));
    }
}

#[test]
fn error_completion_does_not_depend_on_content_length() {
    for length in [None, Some(b"0".as_slice()), Some(b"invalid".as_slice())] {
        assert_error_response(b"401", &[b"token_expired"], length);
        assert_error_response(b"401", &[], length);
    }
}

#[test]
fn empty_errors_with_fin_in_headers_complete_once() {
    for length in [None, Some(b"0".as_slice())] {
        let (mut partial, completed, frames) = partial(0, true);
        assert!(partial.extend_data(headers(0, b"401", length, true)));
        assert!(partial.extend_data(body(0, b"", true)));
        let response = completed.recv_timeout(WAIT).unwrap();
        assert_eq!(response.status_code().unwrap(), b"401");
        assert!(response.as_data().is_empty());
        assert!(completed.try_recv().is_err());
        assert!(frames.try_recv().is_err());
    }
}

#[test]
fn error_body_can_carry_fin_on_its_last_fragment() {
    let (mut partial, completed, frames) = partial(0, true);
    assert!(!partial.extend_data(headers(0, b"401", None, false)));
    assert!(!partial.extend_data(body(0, b"token_", false)));
    assert!(partial.extend_data(body(0, b"expired", true)));
    assert_eq!(
        completed.recv_timeout(WAIT).unwrap().as_data(),
        b"token_expired"
    );
    assert!(frames.try_recv().is_err());
}

#[test]
fn valid_200_notifications_keep_fragmentation_and_multiple_frame_delivery() {
    let (mut partial, completed, frames) = partial(0, true);
    assert!(!partial.extend_data(headers(0, b"200", None, false)));
    let mut wire = b"first-notification".to_vec().prepend_frame().unwrap();
    wire.extend(b"second-notification".to_vec().prepend_frame().unwrap());
    for chunk in wire.chunks(5) {
        assert!(!partial.extend_data(body(0, chunk, false)));
    }
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), b"first-notification");
    assert_eq!(frames.recv_timeout(WAIT).unwrap(), b"second-notification");
    assert!(frames.try_recv().is_err());
    assert!(completed.try_recv().is_err());
}

#[test]
fn informational_100_does_not_replace_the_final_error_status() {
    let (mut partial, completed, frames) = partial(0, true);
    assert!(!partial.extend_data(headers(0, b"100", None, false)));
    assert!(!partial.extend_data(headers(0, b"401", None, false)));
    assert!(partial.extend_data(body(0, b"token_expired", true)));
    assert_eq!(
        completed.recv_timeout(WAIT).unwrap().status_code().unwrap(),
        b"401"
    );
    assert!(frames.try_recv().is_err());
}

#[test]
fn ordinary_non_streamable_error_response_still_completes() {
    let (mut partial, completed, _) = partial(4, false);
    assert!(!partial.extend_data(headers(4, b"401", Some(b"13"), false)));
    assert!(!partial.extend_data(body(4, b"token_", false)));
    assert!(partial.extend_data(body(4, b"expired", true)));
    let response = completed.recv_timeout(WAIT).unwrap();
    assert_eq!(response.status_code().unwrap(), b"401");
    assert_eq!(response.as_data(), b"token_expired");
}

#[test]
fn response_worker_retires_error_and_processes_the_next_response() {
    let controller = ThreadController::new("stream-http-errors-test");
    let channel = ResponseChannel::new(&controller);
    let head = channel.get_head();
    let (error, error_completed, frames) = partial(0, true);
    let (ordinary, ordinary_completed, _) = partial(4, false);
    let table = Arc::new(Mutex::new(HashMap::from([
        ((0, CONNECTION.to_owned()), error),
        ((4, CONNECTION.to_owned()), ordinary),
    ])));
    let (submission, submissions) = bounded(1);
    response_manager_worker::run(
        channel.get_queue(),
        PartialResponseReceiver::new(submissions, &controller),
        &controller,
        &Arc::new(Mutex::new(HashMap::new())),
        table.clone(),
        Arc::new(AtomicU64::new(0)),
    );
    for event in [
        headers(0, b"401", Some(b"13"), false),
        body(0, b"token_", false),
        body(0, b"expired", false),
        body(0, b"", true),
        body(0, b"", true),
        headers(4, b"200", Some(b"2"), false),
        body(4, b"ok", true),
    ] {
        head.send_response(event).unwrap();
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
        ordinary_completed.recv_timeout(WAIT).unwrap().as_data(),
        b"ok"
    );
    assert!(frames.try_recv().is_err());
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(table) = table.try_lock() {
            if table.is_empty() {
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "terminal responses retained in manager"
        );
        std::thread::yield_now();
    }
    drop(head);
    drop(channel);
    drop(submission);
}
