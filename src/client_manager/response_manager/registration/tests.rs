use super::*;
use crate::client_manager::{
    request_manager::HeaderRequest,
    response_manager::{
        response_manager_worker, response_mngr::PartialResponseReceiver, Http3Response,
        ResponseChannel,
    },
};
use crossbeam::channel::{bounded, unbounded};
use quiche::h3::Header;
use std::time::{Duration, Instant};
const WAIT: Duration = Duration::from_secs(1);

struct Fixture {
    table: ResponseTable,
    epoch: Arc<AtomicU64>,
    generation: Arc<AtomicU64>,
    controller: ThreadController,
}
impl Fixture {
    fn new() -> Self {
        Self {
            table: Arc::new(Mutex::new(HashMap::new())),
            epoch: Arc::new(AtomicU64::new(0)),
            generation: Arc::new(AtomicU64::new(1)),
            controller: ThreadController::new("registration-test"),
        }
    }
    fn prepare(&self, streamable: bool) -> PreparedResponse {
        PreparedResponse::new(
            self.table.clone(),
            self.epoch.clone(),
            self.controller.clone(),
            self.generation.clone(),
            "/test",
            vec![],
            streamable.then_some(StreamSub::None),
            Uuid::new_v4(),
        )
    }
}
fn header(prepared: &PreparedResponse) -> (HeaderRequest, Receiver<(u64, String)>) {
    let (sender, receiver) = bounded(1);
    let mut header = HeaderRequest::new(true, sender);
    header.set_registration(prepared.registration());
    (header, receiver)
}

#[test]
fn immediate_response_before_caller_receives_stream_ids_is_not_lost() {
    for streamable in [false, true] {
        let fixture = Fixture::new();
        let channel = ResponseChannel::new(&fixture.controller);
        let head = channel.get_head();
        let (sender, receiver) = unbounded();
        response_manager_worker::run(
            channel.get_queue(),
            PartialResponseReceiver::new(receiver, &fixture.controller),
            &fixture.controller,
            &Arc::new(Mutex::new(HashMap::new())),
            fixture.table.clone(),
            fixture.epoch.clone(),
        );
        let prepared = fixture.prepare(streamable);
        let (header, ids) = header(&prepared);
        // This is the same operation the QUIC loop performs before its next flush.
        header.finish_open(0, "connection", 1).unwrap();
        head.send_response(Http3Response::new_header(
            0,
            "connection".into(),
            vec![Header::new(b":status", b"401")],
            !streamable,
        ))
        .unwrap();
        if streamable {
            head.send_response(Http3Response::new_body_data(
                0,
                "connection".into(),
                b"token_",
                false,
            ))
            .unwrap();
            head.send_response(Http3Response::new_body_data(
                0,
                "connection".into(),
                b"expired",
                true,
            ))
            .unwrap();
        }
        // Deliberately prevent the caller from accepting its id until the whole
        // response has already completed on the real worker.
        let deadline = Instant::now() + WAIT;
        while prepared.response.is_empty() {
            assert!(Instant::now() < deadline, "immediate response lost");
            std::thread::yield_now();
        }
        let waiter = prepared
            .into_waiter(&ids.recv_timeout(WAIT).unwrap())
            .unwrap();
        assert_eq!(
            waiter
                .wait_response_with_timeout(WAIT)
                .unwrap()
                .status_code()
                .unwrap(),
            b"401"
        );
        let deadline = Instant::now() + WAIT;
        while !fixture.table.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(head);
        drop(channel);
        drop(sender);
    }
}

#[test]
fn cloned_ticket_binds_once_without_replacing_its_first_entry() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let registration = prepared.registration();
    let clone = registration.clone();
    let ids = (0, "one".into());
    registration.bind(&ids, 1).unwrap();
    assert!(clone.bind(&(4, "one".into()), 1).is_err());
    assert_eq!(fixture.table.lock().unwrap().len(), 1);
    assert!(fixture.table.lock().unwrap().contains_key(&ids));
    drop(prepared);
    assert!(fixture.table.lock().unwrap().is_empty());
}

#[test]
fn caller_cancellation_before_allocation_prevents_late_registration() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let (header, _) = header(&prepared);
    drop(prepared);
    assert!(!header.can_register_response(1));
    assert!(header.finish_open(0, "one", 1).is_err());
    assert!(fixture.table.lock().unwrap().is_empty());
}

#[test]
fn dropped_confirmation_receiver_retires_the_bound_entry() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let (header, receiver) = header(&prepared);
    drop(receiver);
    assert!(header.finish_open(0, "one", 1).is_err());
    assert!(fixture.table.lock().unwrap().is_empty());
    assert!(prepared.into_waiter(&(0, "one".into())).is_err());
}

#[test]
fn full_confirmation_channel_never_blocks_the_quic_owner() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let (header, _receiver) = header(&prepared);
    header.send_ids(99, "occupied").unwrap();
    assert!(header.finish_open(0, "one", 1).is_err());
    assert!(fixture.table.lock().unwrap().is_empty());
}

#[test]
fn failed_allocation_cancels_without_registering_a_response() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let (header, receiver) = header(&prepared);
    header.cancel_registration();
    drop(header);
    assert!(receiver.recv_timeout(WAIT).is_err());
    assert!(prepared.into_waiter(&(0, "one".into())).is_err());
    assert!(fixture.table.lock().unwrap().is_empty());
}

#[test]
fn retired_epoch_and_connection_generations_cannot_bind() {
    for stale in 0..4 {
        let fixture = Fixture::new();
        let prepared = fixture.prepare(true);
        let (header, _receiver) = header(&prepared);
        match stale {
            0 => {
                fixture.epoch.fetch_add(1, Ordering::AcqRel);
            }
            1 => {
                fixture.generation.fetch_add(1, Ordering::AcqRel);
            }
            2 => fixture.controller.switch_off(),
            _ => {}
        }
        let owner_generation = if stale == 3 { 2 } else { 1 };
        assert!(!header.can_register_response(owner_generation));
        assert!(header.finish_open(0, "one", owner_generation).is_err());
        assert!(fixture.table.lock().unwrap().is_empty());
    }
}

#[test]
fn cleanup_never_removes_another_request_or_a_new_epoch() {
    for newer_epoch in [false, true] {
        let fixture = Fixture::new();
        let old = fixture.prepare(true);
        let ids = (0, "same".into());
        old.registration().bind(&ids, 1).unwrap();
        fixture.table.lock().unwrap().clear();
        if newer_epoch {
            fixture.epoch.fetch_add(1, Ordering::AcqRel);
        }
        let new = fixture.prepare(true);
        new.registration().bind(&ids, 1).unwrap();
        drop(old);
        assert_eq!(
            fixture
                .table
                .lock()
                .unwrap()
                .get(&ids)
                .unwrap()
                .get_request_id(),
            new.registration.request_id
        );
        drop(new);
        assert!(fixture.table.lock().unwrap().is_empty());
    }
}

#[test]
fn equal_stream_numbers_on_different_connections_stay_distinct() {
    let fixture = Fixture::new();
    let one = fixture.prepare(true);
    let two = fixture.prepare(true);
    one.registration().bind(&(0, "one".into()), 1).unwrap();
    two.registration().bind(&(0, "two".into()), 1).unwrap();
    drop(one);
    assert_eq!(fixture.table.lock().unwrap().len(), 1);
    assert!(fixture
        .table
        .lock()
        .unwrap()
        .contains_key(&(0, "two".into())));
    drop(two);
    assert!(fixture.table.lock().unwrap().is_empty());
}

#[test]
fn generation_change_after_confirmation_cancels_before_waiter_acceptance() {
    let fixture = Fixture::new();
    let prepared = fixture.prepare(true);
    let (header, receiver) = header(&prepared);
    header.finish_open(0, "old", 1).unwrap();
    let ids = receiver.recv_timeout(WAIT).unwrap();
    fixture.generation.fetch_add(1, Ordering::AcqRel);
    assert!(prepared.into_waiter(&ids).is_err());
    assert!(fixture.table.lock().unwrap().is_empty());
}
