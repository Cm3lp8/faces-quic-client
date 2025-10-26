use std::sync::{atomic::AtomicBool, Arc};

#[derive(Clone)]
pub struct ThreadController {
    label: String,
    atomic_bool: Arc<AtomicBool>,
}

impl ThreadController {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            atomic_bool: Arc::new(AtomicBool::new(true)),
        }
    }
    pub fn switch_off(&self) {
        self.atomic_bool
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn switch_on(&self) {
        self.atomic_bool
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn is_on(&self) -> bool {
        self.atomic_bool.load(std::sync::atomic::Ordering::Relaxed)
    }
}
