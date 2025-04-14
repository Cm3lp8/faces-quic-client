pub use body_interfaces::{IntoBodyReq, Json};

mod body_interfaces {
    use serde::{Deserialize, Serialize};

    use crate::ContentType;

    pub trait Json {
        fn to_bytes_vec(&self) -> Result<Vec<u8>, serde_json::Error>;
    }

    pub trait IntoBodyReq {
        fn content_type(&self) -> ContentType;
        fn into_bytes(self) -> Vec<u8>;
    }

    impl IntoBodyReq for Vec<u8> {
        fn into_bytes(self) -> Vec<u8> {
            self
        }
        fn content_type(&self) -> ContentType {
            ContentType::OctetStream
        }
    }

    impl<T> IntoBodyReq for T
    where
        T: Serialize + Json,
    {
        fn content_type(&self) -> ContentType {
            ContentType::Json
        }
        fn into_bytes(self) -> Vec<u8> {
            if let Ok(res) = self.to_bytes_vec() {
                res
            } else {
                vec![]
            }
        }
    }
}
