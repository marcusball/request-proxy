use base64;
use serde::ser::{Serialize, Serializer};

pub struct ByteWrapper<T: ?Sized + AsRef<[u8]>>(pub T);

impl<T: ?Sized + AsRef<[u8]>> Serialize for ByteWrapper<T> {
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