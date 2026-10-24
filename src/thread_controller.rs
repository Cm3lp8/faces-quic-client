use std::sync::{Arc, atomic::AtomicU8};

#[derive(Clone)]
pub struct ThreadController {
    label: String,
    atomic_bool: Arc<AtomicU8>,
}

impl ThreadController {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            atomic_bool: Arc::new(AtomicU8::new(0)),
        }
    }
    pub fn switch_off(&self) {
        let _ = self.atomic_bool.compare_exchange(
            0,
            1,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }
    pub fn switch_on(&self) {
        let _ = self.atomic_bool.compare_exchange(
            1,
            0,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }
    /// Terminal transport admission; ordinary reconnects cannot undo this boundary.
    pub(crate) fn seal(&self) {
        self.atomic_bool
            .store(2, std::sync::atomic::Ordering::Release);
    }
    /// Called only when the application admits the next fully isolated owner.
    pub(crate) fn reset(&self) {
        self.atomic_bool
            .store(0, std::sync::atomic::Ordering::Release);
    }
    pub fn is_on(&self) -> bool {
        self.atomic_bool.load(std::sync::atomic::Ordering::Acquire) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_reconnect_cannot_lift_terminal_closure() {
        let controller = ThreadController::new("closure-test");
        let late = controller.clone();
        controller.seal();
        late.switch_on();
        late.switch_off();
        late.switch_on();
        assert!(!controller.is_on());
        controller.reset();
        assert!(controller.is_on());
    }
}
