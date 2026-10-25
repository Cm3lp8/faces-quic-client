use super::{CompletedResponse, PartialResponse, UploadProgressStatus, WaitPeerResponse};
use crate::{
    client_manager::persistant_stream::StreamSub, thread_controller::ThreadController,
    RequestEventListener,
};
use crossbeam::channel::Receiver;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use uuid::Uuid;

type ResponseTable = Arc<Mutex<HashMap<(u64, String), PartialResponse>>>;

enum RegistrationState {
    Pending(PartialResponse),
    Registered((u64, String)),
    Cancelled,
}

/// Shared only by a prepared request and its header clones. Binding consumes the
/// pending response once; cancellation cannot remove another request's entry.
#[derive(Clone)]
pub(crate) struct ResponseRegistration {
    state: Arc<Mutex<RegistrationState>>,
    table: ResponseTable,
    epoch: Arc<AtomicU64>,
    expected_epoch: u64,
    connection_generation: Arc<AtomicU64>,
    expected_generation: u64,
    controller: ThreadController,
    request_id: Uuid,
}
impl std::fmt::Debug for ResponseRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseRegistration")
            .field("request_id", &self.request_id)
            .finish_non_exhaustive()
    }
}
impl ResponseRegistration {
    fn is_current(&self, generation: u64) -> bool {
        self.controller.is_on()
            && self.epoch.load(Ordering::Acquire) == self.expected_epoch
            && generation == self.expected_generation
            && self.connection_generation.load(Ordering::Acquire) == self.expected_generation
    }
    pub(crate) fn is_pending(&self, generation: u64) -> bool {
        self.is_current(generation)
            && matches!(*self.state.lock().unwrap(), RegistrationState::Pending(_))
    }
    /// Called by the QUIC owner after local stream allocation, before either
    /// confirming the id or flushing packets. No callback or channel wait here.
    pub(crate) fn bind(&self, ids: &(u64, String), generation: u64) -> Result<(), ()> {
        let mut state = self.state.lock().unwrap();
        let mut table = self.table.lock().unwrap();
        if !self.is_current(generation)
            || table.contains_key(ids)
            || !matches!(*state, RegistrationState::Pending(_))
        {
            return Err(());
        }
        let RegistrationState::Pending(mut response) =
            std::mem::replace(&mut *state, RegistrationState::Cancelled)
        else {
            return Err(());
        };
        response.bind_stream_ids(ids);
        let path = response.req_path().to_owned();
        table.insert(ids.clone(), response);
        *state = RegistrationState::Registered(ids.clone());
        let size = table.len();
        drop(table);
        drop(state);
        log::trace!("[faces_diag][quic_client][response] waiter_registered source=synchronous path={} request_id={} stream_id={} conn_id={} table_size={}", path, self.request_id, ids.0, ids.1, size);
        Ok(())
    }
    pub(crate) fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        let retired = std::mem::replace(&mut *state, RegistrationState::Cancelled);
        if let RegistrationState::Registered(ids) = &retired {
            let mut table = self.table.lock().unwrap();
            if self.epoch.load(Ordering::Acquire) == self.expected_epoch
                && table
                    .get(ids)
                    .is_some_and(|entry| entry.get_request_id() == self.request_id)
            {
                let response = table.remove(ids);
                drop(table);
                drop(state);
                drop(response);
                return;
            }
        }
        drop(state);
        drop(retired);
    }
}

/// Cancels an unconfirmed request on every early-return path. Once ids have
/// been confirmed the existing response manager owns its lifetime.
pub(crate) struct PreparedResponse {
    registration: ResponseRegistration,
    response: Receiver<CompletedResponse>,
    progress: Receiver<UploadProgressStatus>,
    confirmed: bool,
}
impl PreparedResponse {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        table: ResponseTable,
        epoch: Arc<AtomicU64>,
        controller: ThreadController,
        connection_generation: Arc<AtomicU64>,
        path: &str,
        subscribers: Vec<Arc<dyn RequestEventListener + Send + Sync>>,
        stream: Option<StreamSub>,
        request_id: Uuid,
    ) -> Self {
        // The placeholder is private and never enters the response table.
        let ids = (0, String::new());
        let (partial, response, progress) = match stream {
            Some(stream) => {
                PartialResponse::new_streamable(path, subscribers, stream, &ids, request_id)
            }
            None => PartialResponse::new(path, subscribers, &ids, request_id),
        };
        let registration = ResponseRegistration {
            state: Arc::new(Mutex::new(RegistrationState::Pending(partial))),
            table,
            expected_epoch: epoch.load(Ordering::Acquire),
            epoch,
            expected_generation: connection_generation.load(Ordering::Acquire),
            connection_generation,
            controller,
            request_id,
        };
        Self {
            registration,
            response,
            progress,
            confirmed: false,
        }
    }
    pub(crate) fn registration(&self) -> ResponseRegistration {
        self.registration.clone()
    }
    pub(crate) fn into_waiter(mut self, ids: &(u64, String)) -> Result<WaitPeerResponse, ()> {
        {
            let state = self.registration.state.lock().unwrap();
            if !self
                .registration
                .is_current(self.registration.expected_generation)
                || !matches!(&*state, RegistrationState::Registered(bound) if bound == ids)
            {
                return Err(());
            }
        }
        self.confirmed = true;
        Ok(WaitPeerResponse::new(
            ids,
            self.response.clone(),
            self.progress.clone(),
            &self.registration.controller,
        ))
    }
}
impl Drop for PreparedResponse {
    fn drop(&mut self) {
        if !self.confirmed {
            self.registration.cancel();
        }
    }
}

#[cfg(test)]
mod tests;
