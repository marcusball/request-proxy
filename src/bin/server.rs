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
use std::collections::VecDeque;

use futures::Stream;
use futures::{Async, Future, Poll};

use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};

use uuid::Uuid;

use dotenv::dotenv;

const PHRASE: &'static str = "OK";


struct OkFuture;
impl Future for OkFuture {
    type Item = <RequestProxy as Service>::Response;
    type Error = <RequestProxy as Service>::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        Ok(Async::Ready(
            Response::new()
                .with_header(ContentLength(PHRASE.len() as u64))
                .with_body(PHRASE),
        ))
    }
}


struct RequestProxy {
    requests: Arc<Mutex<VecDeque<Request<::hyper::Body>>>>,
}

impl Service for RequestProxy {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        println!("{}", &req.uri());

        self.requests.lock().unwrap().push_back(req);

        Box::new(OkFuture)
    }
}

struct ProxyOutput {
    requests: Arc<Mutex<VecDeque<Request<::hyper::Body>>>>,
}

impl Service for ProxyOutput {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, _: Request) -> Self::Future {
        let req = self.requests.lock().unwrap().pop_front();

        if req.is_none() {
            return Box::new(futures::future::ok(Response::new().with_body("NONE")));
        }

        let (method, uri, version, headers, body) = req.unwrap().deconstruct();

        Box::new(
            body.fold(Vec::new(), |mut acc, chunk| {
                acc.extend_from_slice(chunk.as_ref());
                Ok::<_, hyper::Error>(acc)
            }).and_then(move |bytes| {
                    let output = ProxiedRequest {
                        id: Uuid::new_v4(),
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

    let child = thread::spawn(move || {
        let server = Http::new()
            .bind(&in_addr, move || {
                Ok(RequestProxy {
                    requests: request_log.clone(),
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
                })
            })
            .unwrap();
        server2.run().unwrap();
    });

    let _ = child.join();
    let _ = child2.join();
}
