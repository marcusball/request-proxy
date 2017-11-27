extern crate base64;
extern crate dotenv;
extern crate futures;
extern crate hyper;
extern crate request_proxy;
extern crate serde;
extern crate serde_json;
extern crate uuid;

use request_proxy::types::*;

use std::env;
use std::thread;
use std::sync::{Arc, Mutex};
use std::collections::{HashMap, VecDeque};

use futures::Stream;
use futures::{Async, Future, Poll};

use hyper::server::{Http, Request, Response, Service};
use hyper::{Method, StatusCode};

use uuid::Uuid;

use dotenv::dotenv;


struct ProxiedResponse {
    request_id: Uuid,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl Future for ProxiedResponse {
    type Item = <RequestProxy as Service>::Response;
    type Error = <RequestProxy as Service>::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.responses
            .try_lock()
            .and_then(|mut res| match res.remove(&self.request_id) {
                Some(response) => {
                    println!("Response found!");
                    Ok(Async::Ready(response))
                },
                None => {
                    futures::task::current().notify();
                    Ok(Async::NotReady)
                },
            })
            .or_else(|_| {
                println!("Blocked :(");
                futures::task::current().notify();
                Ok(Async::NotReady)
            })
    }
}


struct RequestProxy {
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl Service for RequestProxy {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        println!("{}", &req.uri());

        let request_id = Uuid::new_v4();

        self.requests
            .lock()
            .unwrap()
            .push_back((request_id.clone(), req));

        Box::new(ProxiedResponse {
            request_id,
            responses: self.responses.clone(),
        })
    }
}

struct ProxyOutput {
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl ProxyOutput {
    /// Pop a queued request, if any, and return the serialized request
    fn pop_request(&self) -> <Self as Service>::Future {
        let req = { self.requests.lock().unwrap().pop_front() };

        if req.is_none() {
            return Box::new(futures::future::ok(Response::new().with_body("NONE")));
        }

        let (req_id, req) = req.unwrap();
        let (method, uri, version, headers, body) = req.deconstruct();

        Box::new(
            body.fold(Vec::new(), |mut acc, chunk| {
                acc.extend_from_slice(chunk.as_ref());
                Ok::<_, hyper::Error>(acc)
            }).and_then(move |bytes| {
                    let output = ProxiedRequest {
                        id: req_id,
                        method: method.as_ref(),
                        uri: uri.as_ref(),
                        version: format!("{}", version),
                        headers: headers
                            .iter()
                            .map(|header| {
                                (
                                    header.name(),
                                    Base64Bytes(header.raw().into_iter().fold(
                                        Vec::new(),
                                        |mut acc, bytes| {
                                            acc.extend_from_slice(bytes);
                                            acc
                                        },
                                    )),
                                )
                            })
                            .collect(),
                        body: Base64Bytes(bytes),
                    };

                    futures::future::ok(
                        Response::new().with_body(serde_json::to_string(&output).unwrap()),
                    )
                }),
        )
    }

    fn push_response(&self, request: Request) -> <Self as Service>::Future {
        println!("Received client POST response");
        let responses = self.responses.clone();

        Box::new(
            request
                .body()
                .fold(Vec::new(), |mut acc, chunk| {
                    acc.extend_from_slice(chunk.as_ref());
                    Ok::<_, hyper::Error>(acc)
                })
                .map(move |bytes| String::from_utf8(bytes).unwrap())
                .and_then(move |body| {
                    let client_response = match serde_json::from_str::<ClientResponse>(&body) {
                        Ok(r) => r,
                        Err(_) => {
                            return futures::future::ok(
                                Response::new()
                                    .with_status(StatusCode::BadRequest)
                                    .with_body("🤢 Your request was bad and you should feel bad"),
                            );
                        }
                    };

                    let response = Response::<hyper::Body>::new()
                        .with_status(client_response.status_code())
                        .with_headers(client_response.headers())
                        .with_body(client_response.body.as_str().unwrap_or("").to_owned());

                    // TODO Check requests to verify the request ID is actually present

                    match responses.lock().and_then(|mut responses| {
                        let _ = responses.insert(client_response.request_id, response);
                        Ok(())
                    }) {
                        Err(_) => {
                            return futures::future::ok(
                                Response::new()
                                    .with_status(StatusCode::InternalServerError)
                                    .with_body("🤒 Server machine broke"),
                            );
                        }
                        Ok(_) => { 
                            /* 👍 */
                            println!("Response to {} was successfully saved", client_response.request_id.hyphenated().to_string());
                        }
                    };

                    // Update so that the ProxiedResponse future can continue 
                    futures::task::current().notify();

                    futures::future::ok(
                        Response::new()
                            .with_body(client_response.request_id.hyphenated().to_string()),
                    )
                }),
        )
    }
}

impl Service for ProxyOutput {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, request: Request) -> Self::Future {
        match request.method() {
            &Method::Get => self.pop_request(),
            &Method::Post => self.push_response(request),
            _ => Box::new(futures::future::ok(
                Response::new()
                    .with_status(StatusCode::MethodNotAllowed)
                    .with_body("😬 You suck at computers"),
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
    let request_log_clone = request_log.clone();

    let response_log = Arc::new(Mutex::new(HashMap::new()));
    let response_log_clone = response_log.clone();

    let child = thread::spawn(move || {
        let server = Http::new()
            .bind(&in_addr, move || {
                Ok(RequestProxy {
                    requests: request_log.clone(),
                    responses: response_log.clone(),
                })
            })
            .unwrap();
        server.run().unwrap();
    });

    let child2 = thread::spawn(move || {
        let server2 = Http::new()
            .bind(&out_addr, move || {
                Ok(ProxyOutput {
                    requests: request_log_clone.clone(),
                    responses: response_log_clone.clone(),
                })
            })
            .unwrap();
        server2.run().unwrap();
    });

    let _ = child.join();
    let _ = child2.join();
}
