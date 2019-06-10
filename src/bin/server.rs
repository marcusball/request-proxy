extern crate base64;
extern crate dotenv;
extern crate futures;
extern crate hyper;
extern crate request_proxy;
extern crate serde;
extern crate serde_json;
extern crate tokio_timer;
extern crate uuid;
#[macro_use]
extern crate failure;
extern crate rand;

use request_proxy::types::*;

use std::collections::{HashMap, VecDeque};
use std::env;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::*;

use futures::future::Either;
use futures::Stream;
use futures::{Async, Poll};
use tokio_timer::Timeout;

use hyper::rt::{self, Future};
use hyper::service::service_fn;
use hyper::{Body, Method, Server, StatusCode};
use hyper::{Request, Response};

use failure::Fail;
use rand::Rng;
use uuid::Uuid;

use dotenv::dotenv;

pub mod error {
    use std::convert::From;
    pub use tokio_timer::timeout::Error as TimeoutError;

    #[derive(Debug, Fail)]
    pub enum Error {
        #[fail(display = "Timeout connecting to gateway")]
        TokioTimeoutError(),

        #[fail(display = "Hyper error: {}", _0)]
        HyperError(hyper::Error),
    }

    impl From<TimeoutError<Self>> for Error {
        fn from(_: TimeoutError<Self>) -> Self {
            Error::TokioTimeoutError()
        }
    }

    impl From<hyper::Error> for Error {
        fn from(e: hyper::Error) -> Self {
            Error::HyperError(e)
        }
    }
}

struct ProxiedResponse {
    request_id: Uuid,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl Future for ProxiedResponse {
    type Item = Response<Body>;
    type Error = error::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.responses
            .try_lock()
            .and_then(|mut res| match res.remove(&self.request_id) {
                Some(response) => {
                    println!("Response found!");
                    Ok(Async::Ready(response))
                }
                None => {
                    futures::task::current().notify();
                    Ok(Async::NotReady)
                }
            })
            .or_else(|_| {
                futures::task::current().notify();
                Ok(Async::NotReady)
            })
    }
}

#[derive(Clone)]
struct RequestProxy {
    secret: String,
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl RequestProxy {
    fn call(&self, req: Request<Body>) -> impl Future<Item = Response<Body>, Error = error::Error> {
        // Check if the Client read header is present, and if so, get the value.
        match req.headers().get("x-proxy-secret").map(|h| h.to_str()) {
            // If the value is not present, this is an external request to be forwarded to the client.
            // Push the request to the queue for the client.
            None => Either::A(Either::A(self.push_request(req))),

            // If a secret key header was sent, and the key is correct,
            // then handle the authenticated client's request (forward a request, or receive a response).
            Some(Ok(key)) if key == self.secret => {
                Either::A(Either::B(self.handle_proxy_client_request(req)))
            }

            // If the secret key header was sent, but the key is incorrect.
            Some(Ok(x)) => {
                println!("Incorrect secret key '{}'!", x);

                Either::B(futures::future::ok(
                    Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .header("content-type", "text/plain; charset=utf-8")
                        .body(Body::from("🐸 GET OUT".to_string()))
                        .unwrap(),
                ))
            }

            // If the secret key header was sent, but we failed to read the value as a String.
            Some(Err(e)) => {
                eprintln!("Error decoding Secret Key: {}", e);

                Either::B(futures::future::ok(
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("content-type", "text/plain; charset=utf-8")
                        .body(Body::from(
                            "🤢 Your request was bad and you should feel bad",
                        ))
                        .unwrap(),
                ))
            }
        }
    }

    fn handle_proxy_client_request(
        &self,
        request: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = error::Error> {
        match request.method() {
            &Method::GET => Either::A(Either::A(self.pop_request())),
            &Method::POST => Either::A(Either::B(self.push_response(request))),
            _ => Either::B(futures::future::ok(
                Response::builder()
                    .status(StatusCode::METHOD_NOT_ALLOWED)
                    .header("content-type", "text/plain")
                    .body(Body::from("😬 You suck at computers"))
                    .unwrap(),
            )),
        }
    }

    fn push_request(
        &self,
        req: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = error::Error> {
        println!("{}", &req.uri());

        let request_id = Uuid::new_v4();

        self.requests
            .lock()
            .expect("Failed to lock requests queue!")
            .push_back((request_id.clone(), req));

        let await_response = ProxiedResponse {
            request_id,
            responses: self.responses.clone(),
        };

        let timeout_response: Response<Body> = Response::builder()
            .status(504)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from("😶 Timeout".to_string()))
            .unwrap();

        let requests_clone = self.requests.clone();

        Timeout::new(await_response, Duration::from_secs(15))
            .map_err(|e| error::Error::from(e))
            .or_else(move |_| {
                // If the request timed out, remove it from the queue.
                Self::remove_request(request_id, requests_clone);

                Ok(timeout_response)
            })
    }

    /// Remove a request from the request queue
    fn remove_request(
        request_id: Uuid,
        queue: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    ) {
        // Find the index of the request
        let mut requests = queue.lock().expect("Failed to lock the requests queue!");

        let index = requests.iter().rposition(|req| req.0 == request_id);

        if let Some(i) = index {
            requests.remove(i);
        }
    }

    /// Pop a queued request, if any, and return the serialized request
    fn pop_request(&self) -> impl Future<Item = Response<Body>, Error = error::Error> {
        let req = {
            self.requests
                .lock()
                .expect("Failed to lock request queue to pop request")
                .pop_front()
        };

        if req.is_none() {
            return Either::A(futures::future::ok(
                Response::builder()
                    .status(StatusCode::NO_CONTENT)
                    .body(Body::empty())
                    .unwrap(),
            ));
        }

        let (req_id, req) = req.expect("Failed to unwrap queued Request");
        let (parts, body) = req.into_parts();

        Either::B(
            body.fold(Vec::new(), |mut acc, chunk| {
                acc.extend_from_slice(chunk.as_ref());
                Ok::<_, hyper::Error>(acc)
            })
            .map_err(|e| error::Error::from(e))
            .and_then(move |bytes| {
                let output = ProxiedRequest {
                    id: req_id,
                    method: parts.method.as_ref(),
                    uri: parts.uri.to_string(),
                    version: format!("{:?}", parts.version),
                    headers: parts
                        .headers
                        .iter()
                        .map(|(name, value)| {
                            (name.as_str(), Base64Bytes(value.as_bytes().to_vec()))
                        })
                        .collect(),
                    body: Base64Bytes(bytes),
                };

                futures::future::ok(
                    Response::builder()
                        .body(Body::from(
                            serde_json::to_string(&output).expect("Failed to serialize to JSON"),
                        ))
                        .unwrap(),
                )
            }),
        )
    }

    fn push_response(
        &self,
        request: Request<Body>,
    ) -> impl Future<Item = Response<Body>, Error = error::Error> {
        println!("Received client POST response");
        let responses = self.responses.clone();

        request
            .into_body()
            .fold(Vec::new(), |mut acc, chunk| {
                acc.extend_from_slice(chunk.as_ref());
                Ok::<_, hyper::Error>(acc)
            })
            .map_err(|e| error::Error::from(e))
            .map(move |bytes| String::from_utf8(bytes).expect("Failed to create string from bytes"))
            .and_then(move |body| {
                let client_response = match serde_json::from_str::<ClientResponse>(&body) {
                    Ok(r) => r,
                    Err(_) => {
                        return futures::future::ok(
                            Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .header("content-type", "text/plain; charset=utf-8")
                                .body(Body::from(
                                    "🤢 Your request was bad and you should feel bad",
                                ))
                                .unwrap(),
                        );
                    }
                };

                let mut response = Response::builder()
                    .status(client_response.status_code())
                    .body(Body::from(
                        client_response.body.as_str().unwrap_or("").to_owned(),
                    ))
                    .unwrap(); // TODO: Remove unwrap call

                {
                    // Scope to make the borrow checker happy
                    let headers = response.headers_mut();
                    for (name, value) in client_response.headers().into_iter() {
                        if let Some(name) = name {
                            headers.append(name, value);
                        } else {
                            eprintln!("Somehow received a header will no name?");
                        }
                    }
                }

                let response = response;

                // TODO Check requests to verify the request ID is actually present

                match responses.lock().and_then(|mut responses| {
                    let _ = responses.insert(client_response.request_id, response);
                    Ok(())
                }) {
                    Err(_) => {
                        return futures::future::ok(
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header("content-type", "text/plain; charset=utf-8")
                                .body(Body::from("🤒 Server machine broke"))
                                .unwrap(),
                        );
                    }
                    Ok(_) => {
                        /* 👍 */
                        println!(
                            "Response to {} was successfully saved",
                            client_response.request_id.to_hyphenated().to_string()
                        );
                    }
                };

                // Update so that the ProxiedResponse future can continue
                futures::task::current().notify();

                futures::future::ok(
                    Response::builder()
                        .body(Body::from(
                            client_response.request_id.to_hyphenated().to_string(),
                        ))
                        .unwrap(),
                )
            })
    }
}

fn main() {
    dotenv().ok();

    // Read the port on which to listen.
    let port = u16::from_str(&std::env::var("PORT").unwrap_or("3000".into()))
        .expect("Failed to parse $PORT!");

    // Read the IP address on which to listen
    let ip = std::net::IpAddr::from_str(&std::env::var("LISTEN_IP").unwrap_or("127.0.0.1".into()))
        .expect("Failed to parse $LISTEN_IP");

    // Construct the full Socket address
    let listen_addr = std::net::SocketAddr::new(ip, port);

    // Get the configured $PROXY_SECRET or generate a one-time random key.
    let secret = env::var("PROXY_SECRET")
        .unwrap_or_else(|_| base64::encode(&rand::thread_rng().gen::<[u8; 30]>()));

    println!("Using '{}' as proxy secret key.", secret);

    let request_log = Arc::new(Mutex::new(VecDeque::new()));
    let response_log = Arc::new(Mutex::new(HashMap::new()));

    rt::run(rt::lazy(move || {
        let proxy = RequestProxy {
            secret: secret,
            requests: request_log.clone(),
            responses: response_log.clone(),
        };

        let in_srv = Server::bind(&listen_addr)
            .serve(move || {
                let proxy_clone = proxy.clone();

                service_fn(move |request| proxy_clone.call(request).map_err(|e| e.compat()))
            })
            .map_err(|e| eprintln!("Server 1 error: {}", e));

        println!("Listening on http://{}", listen_addr);

        rt::spawn(in_srv);

        Ok(())
    }));
}
