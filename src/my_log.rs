use std::fmt::Debug;
use std::fs::{self, File, OpenOptions};
use std::io::Write;

pub fn init() {
    if let Ok(ex) = fs::exists("/home/camille/Documents/rust/faces-quic-client/log.text") {
        if ex {
            fs::remove_file("/home/camille/Documents/rust/faces-quic-client/log.text");
        }
    }
}
pub fn debug(debug: impl Debug) {
    log(format!("[{:#?}]", debug).as_str());
}
pub fn log(s: &str) {
    let mut open_options = OpenOptions::new();
    if let Ok(mut file) = open_options
        .create(true)
        .append(true)
        .open("/home/camille/Documents/rust/faces-quic-client/log.text")
    {
        writeln!(file, "{}", s);
    }
}
