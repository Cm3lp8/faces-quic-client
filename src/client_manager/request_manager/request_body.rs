pub use content_type::ContentType;
pub use rqst_body::RequestBody;

mod content_type {
    ///
    /// Information about data type.
    ///
    pub enum ContentType {
        TextPlain,
        TextHtml,
        TextCSS,
        ImageJpeg,
        ImageGif,
        ImagePng,
        Json,
        VideoMp4,
        AudioWav,
        AudioMp3,
        AudioOgg,
        AudioFlac,
        Custom(String),
        Zip,
        Rar,
        OctetStream,
    }

    impl ContentType {
        pub fn to_string(&self) -> String {
            match self {
                Self::TextPlain => String::from("text/plain"),
                Self::TextHtml => String::from("text/html"),
                Self::TextCSS => String::from("text/css"),
                Self::ImageJpeg => String::from("image/jpeg"),
                Self::ImageGif => String::from("image/gif"),
                Self::ImagePng => String::from("image/png"),
                Self::Json => String::from("application/json"),
                Self::VideoMp4 => String::from("video/mp4"),
                Self::AudioWav => String::from("audio/wav"),
                Self::AudioMp3 => String::from("audio/mp3"),
                Self::AudioOgg => String::from("audio/ogg"),
                Self::AudioFlac => String::from("audio/flac"),
                Self::Custom(custom_type) => format!("custom/{}", custom_type),
                Self::Zip => String::from("application/zip"),
                Self::Rar => String::from("application/rar"),
                Self::OctetStream => String::from("application/octet-stream"),
            }
        }
    }
}

mod rqst_body {
    use core::panic;
    use std::{
        fs::{self, File, OpenOptions},
        io::{BufRead, BufReader, BufWriter, Cursor, Error, ErrorKind, Read},
        path::{Path, PathBuf},
    };

    use log::{error, warn};

    pub enum RequestBody {
        Data(BufReader<Cursor<Vec<u8>>>),
        File(BufReader<File>),
        Stream(Box<dyn Read + Send + 'static>),
        Empty,
    }

    impl RequestBody {
        pub fn new_data(data: Vec<u8>) -> RequestBody {
            RequestBody::Data(BufReader::new(Cursor::new(data)))
        }
        pub fn new_file_path(path: PathBuf) -> RequestBody {
            if let Ok(c) = fs::exists(path.as_path()) {
                if c {
                } else {
                    error!("file [{:?}] doesn't exist", path.as_path());
                }
            } else {
                error!("Error trying to find if file exists");
            }
            let file = match File::open(path.as_path()) {
                Ok(f) => f,
                Err(e) => {
                    error!("[{:?}]", e);
                    panic!("pb");
                }
            };
            RequestBody::File(BufReader::new(file))
        }
        pub fn new_stream(stream: Box<dyn Read + Send + 'static>) -> RequestBody {
            RequestBody::Stream(stream)
        }
        pub fn len(&self) -> usize {
            match self {
                Self::Data(data) => data.get_ref().get_ref().len(),
                Self::File(file) => file.get_ref().metadata().unwrap().len() as usize,
                Self::Stream(stream) => 0,
                Self::Empty => 0,
            }
        }
        pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, Error> {
            match self {
                Self::Empty => Err(Error::other("empty payload")),
                Self::Data(data) => data.read(buf),
                Self::File(file) => file.read(buf),
                Self::Stream(stream) => stream.read(buf),
            }
        }
    }

    impl PartialEq for RequestBody {
        fn eq(&self, other: &Self) -> bool {
            match self {
                Self::Data(_) => {
                    if let Self::Data(_) = other {
                        true
                    } else {
                        false
                    }
                }
                Self::File(_) => {
                    if let Self::File(_) = other {
                        true
                    } else {
                        false
                    }
                }
                Self::Stream(_) => {
                    if let Self::Stream(_) = other {
                        true
                    } else {
                        false
                    }
                }
                Self::Empty => true,
            }
        }
    }
}
