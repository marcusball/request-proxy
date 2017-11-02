extern crate futures;
extern crate hyper;

use std::thread;
use std::sync::{Arc, Mutex};

use futures::future::Future;
use futures::Stream;

use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};
use hyper::Chunk;

const PHRASE: &'static str = "Hello, world!";

struct RequestProxy {
    requests: Arc<Mutex<Vec<Request<::hyper::Body>>>>
}

impl Service for RequestProxy {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        self.requests.lock().unwrap().push(req);

        Box::new(
            futures::future::ok(
                Response::new()
                    .with_header(ContentLength(PHRASE.len() as u64))
                    .with_body(PHRASE),
            )
        )
    }
} 


fn main() {
    let in_addr = "127.0.0.1:3000".parse().unwrap();
    let out_addr = "127.0.0.1:3001".parse().unwrap();

    let request_log =  Arc::new(Mutex::new(Vec::new()));
    let request_log_clone = request_log.clone();

    let child = thread::spawn(move || {
        let server = Http::new().bind(&in_addr, move || Ok(RequestProxy { requests: request_log.clone() })).unwrap();
        server.run().unwrap();
    });

    let child2 = thread::spawn(move || {
        let server2 = Http::new().bind(&out_addr, move || Ok(RequestProxy { requests: request_log_clone.clone() })).unwrap();
        server2.run().unwrap();
    });

    let _ = child.join();
    let _ = child2.join();
}
