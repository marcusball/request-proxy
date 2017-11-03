use base64;
use serde::ser::{Serialize, Serializer};

/// Wraps a type that may be expressed as a byte slice, 
pub struct Base64Bytes<T: ?Sized + AsRef<[u8]>>(pub T);

impl<T: ?Sized + AsRef<[u8]>> Serialize for Base64Bytes<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error> where S: Serializer {
        serialize_base64(&self.0, serializer)
    }
}

fn serialize_base64<'a, T, S>(field: &T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: ?Sized + AsRef<[u8]>,
{
    base64::encode(field).serialize(serializer)
}


#[derive(Serialize)]
pub struct ProxiedRequest<'a> {
    pub method: &'a str,
    pub uri: &'a str,
    pub version: String,
    pub headers: Vec<(&'a str, Base64Bytes<Vec<u8>>)>,
    pub body: Base64Bytes<&'a [u8]>,
}