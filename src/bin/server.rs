extern crate request_proxy;
extern crate base64;
extern crate futures;
extern crate hyper;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

use request_proxy::types::ByteWrapper;

use std::thread;
use std::sync::{Arc, Mutex};

use futures::future::Future;
use futures::Stream;

use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};
use hyper::Chunk;

const PHRASE: &'static str = "OK";

struct RequestProxy {
    requests: Arc<Mutex<Vec<Request<::hyper::Body>>>>,
}

impl Service for RequestProxy {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        println!("{}", &req.uri());

        self.requests.lock().unwrap().push(req);

        Box::new(futures::future::ok(
            Response::new()
                .with_header(ContentLength(PHRASE.len() as u64))
                .with_body(PHRASE),
        ))
    }
}

struct ProxyOutput {
    requests: Arc<Mutex<Vec<Request<::hyper::Body>>>>,
}

impl Service for ProxyOutput {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, _: Request) -> Self::Future {
        let req = self.requests.lock().unwrap().pop();

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
                        method: method.as_ref(),
                        uri: uri.as_ref(),
                        version: format!("{}", version),
                        headers: headers
                            .iter()
                            .map(|header| {
                                (
                                    header.name(),
                                    ByteWrapper(header.raw().into_iter().fold(Vec::new(), |mut acc, bytes| {
                                        acc.extend_from_slice(bytes);
                                        acc
                                    })),
                                )
                            })
                            .collect(),
                        body: ByteWrapper(bytes.as_ref()),
                    };

                    futures::future::ok(
                        Response::new().with_body(serde_json::to_string(&output).unwrap()),
                    )
                }),
        )
    }
}

#[derive(Serialize)]
struct ProxiedRequest<'a> {
    method: &'a str,
    uri: &'a str,
    version: String,
    headers: Vec<(&'a str, ByteWrapper<Vec<u8>>)>,
    body: ByteWrapper<&'a [u8]>,
}

fn main() {
    let in_addr = "127.0.0.1:3000".parse().unwrap();
    let out_addr = "127.0.0.1:3001".parse().unwrap();

    let request_log = Arc::new(Mutex::new(Vec::new()));
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
