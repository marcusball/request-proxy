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

use request_proxy::types::*;

use std::collections::{HashMap, VecDeque};
use std::env;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::*;

use futures::future::Either;
use futures::{Async, Poll};
use futures::{IntoFuture, Stream};
use tokio_timer::Timeout;

use hyper::rt::{self, Future};
use hyper::server::conn::Http;
use hyper::service::{service_fn, service_fn_ok, Service};
use hyper::{Body, Method, Server, StatusCode};
use hyper::{Request, Response};

use failure::Fail;
use uuid::Uuid;

use dotenv::dotenv;

pub mod error {
    use super::ProxiedResponse;
    use failure::Error as FailureError;
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
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl RequestProxy {
    fn call(&self, req: Request<Body>) -> impl Future<Item = Response<Body>, Error = error::Error> {
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

        Timeout::new(await_response, Duration::from_secs(15))
            .map_err(|e| error::Error::from(e))
            .or_else(|_| Ok(timeout_response))
    }
}

#[derive(Clone)]
struct ProxyOutput {
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl ProxyOutput {
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
                Response::builder().body(Body::from("NONE")).unwrap(),
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

impl ProxyOutput {
    fn call(
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
}

fn main() {
    dotenv().ok();

    let in_addr = env::var("PROXY_LISTEN_IN")
        .unwrap_or("127.0.0.1:3000".into())
        .parse()
        .unwrap();
    let out_addr = env::var("PROXY_LISTEN_OUT")
        .unwrap_or("127.0.0.1:3001".into())
        .parse()
        .unwrap();

    let request_log = Arc::new(Mutex::new(VecDeque::new()));
    let response_log = Arc::new(Mutex::new(HashMap::new()));

    rt::run(rt::lazy(move || {
        let proxy = RequestProxy {
            requests: request_log.clone(),
            responses: response_log.clone(),
        };

        let output = ProxyOutput {
            requests: request_log.clone(),
            responses: response_log.clone(),
        };

        let in_srv = Server::bind(&in_addr)
            .serve(move || {
                let proxy_clone = proxy.clone();

                service_fn(move |request| proxy_clone.call(request).map_err(|e| e.compat()))
            })
            .map_err(|e| eprintln!("Server 1 error: {}", e));

        let out_srv = Server::bind(&out_addr)
            .serve(move || {
                let output_clone = output.clone();

                service_fn(move |request| output_clone.call(request).map_err(|e| e.compat()))
            })
            .map_err(|e| eprintln!("Server 2 error: {}", e));

        println!("Listening on http://{} and http://{}", in_addr, out_addr);

        rt::spawn(in_srv);
        rt::spawn(out_srv);

        Ok(())
    }));
}
