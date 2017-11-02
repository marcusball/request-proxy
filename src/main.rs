extern crate futures;
extern crate hyper;

use futures::future::Future;
use futures::Stream;

use hyper::header::ContentLength;
use hyper::server::{Http, Request, Response, Service};
use hyper::Chunk;

const PHRASE: &'static str = "Hello, world!";

struct RequestProxy;

impl Service for RequestProxy {
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;

    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        let (method, uri, version, headers, body) = req.deconstruct();

        Box::new(
            // Get the request body stream
            body
                // Fold it into a Vec<u8>
                .fold(Vec::new(), |mut acc, chunk| {
                    acc.extend_from_slice(chunk.as_ref());
                    Ok::<_, hyper::Error>(acc)
                })
                // Convert the Vec to a String
                .map(move |value| String::from_utf8(value).unwrap())
                // Echo the body and return a basic response
                .and_then(move |body| {
                    println!("{} {} {}\n{}\n{}", &method, &uri, version, headers, body);

                    futures::future::ok(
                        Response::new()
                            .with_header(ContentLength(PHRASE.len() as u64))
                            .with_body(PHRASE),
                    ) 
                }),
        )
    }
}


fn main() {
    let addr = "127.0.0.1:3000".parse().unwrap();
    let server = Http::new().bind(&addr, || Ok(RequestProxy)).unwrap();
    server.run().unwrap();
}
