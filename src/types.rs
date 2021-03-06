use base64;
use serde::de::{self, Deserialize, Deserializer, Visitor};
use serde::ser::{Serialize, Serializer};
use uuid::Uuid;

use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::StatusCode;

/// Wraps a type that may be expressed as a byte slice,
pub struct Base64Bytes<T: ?Sized + AsRef<[u8]>>(pub T);

impl Base64Bytes<Vec<u8>> {
    // Convert the bytes to a str using UTF-8 encoding
    pub fn as_str(&self) -> Result<&str, ::std::str::Utf8Error> {
        ::std::str::from_utf8(&self.0)
    }
}

impl<T: ?Sized + AsRef<[u8]>> Serialize for Base64Bytes<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        base64::encode(&self.0).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Base64Bytes<Vec<u8>> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer
            .deserialize_string(Base64Visitor::<Vec<u8>>::new())
            .map(|d| Base64Bytes(d))
    }
}

/// Visitor struct for deserializing Base64-encoded strings using Serde
struct Base64Visitor<T: ?Sized + AsRef<[u8]>>(::std::marker::PhantomData<T>);

impl<T: ?Sized + AsRef<[u8]>> Base64Visitor<T> {
    fn new() -> Base64Visitor<T> {
        Base64Visitor(::std::marker::PhantomData)
    }
}

impl<'de> Visitor<'de> for Base64Visitor<Vec<u8>> {
    type Value = Vec<u8>;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        formatter.write_str("a base64 encoded string")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(base64::decode(v).unwrap())
    }

    fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(base64::decode(v).unwrap())
    }

    fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(base64::decode(&v).unwrap())
    }
}

#[derive(Serialize, Deserialize)]
pub struct ProxiedRequest<'a> {
    pub method: &'a str,
    pub uri: String,
    pub version: String,
    pub headers: Vec<(&'a str, Base64Bytes<Vec<u8>>)>,
    pub body: Base64Bytes<Vec<u8>>,
    pub id: Uuid,
}

#[derive(Serialize, Deserialize)]
pub struct ClientResponse {
    /// ID of the ProxiedRequest to which this is the response
    pub request_id: Uuid,
    pub status: u16,
    pub headers: Vec<(String, Base64Bytes<Vec<u8>>)>,
    pub body: Base64Bytes<Vec<u8>>,
}

impl ClientResponse {
    pub fn headers(&self) -> HeaderMap {
        self.headers
            .iter()
            .fold(HeaderMap::new(), |mut headers, &(ref k, ref v)| {
                let name_bytes: &[u8] = k.as_ref();
                let value_bytes: &[u8] = v.0.as_ref();

                let name = HeaderName::from_bytes(name_bytes);
                let value = HeaderValue::from_bytes(value_bytes);

                match (name, value) {
                    (Ok(name), Ok(value)) => {
                        headers.append(name, value);
                    }
                    (Err(e), Ok(_)) => {
                        println!("ERROR: Invalid header name '{}'! Message: {}", k, e);
                    }
                    (Ok(_), Err(e)) => {
                        println!("ERROR: Invalid value for header '{}'! Message: {}", k, e);
                    }
                    (Err(e1), Err(e2)) => {
                        println!("ERROR: This whole header is fucked up: '{}'! \nMessage 1: {}\n Message 2: {}", k, e1, e2);
                    }
                }

                headers
            })
    }

    pub fn status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.status).unwrap_or(StatusCode::BAD_GATEWAY)
    }
}
