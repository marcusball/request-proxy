extern crate base64;
extern crate dotenv;
extern crate futures;
extern crate hyper;
extern crate request_proxy;
extern crate serde;
extern crate serde_json;
extern crate uuid;
#[macro_use]
extern crate failure;
extern crate rand;
extern crate tokio;

use request_proxy::types::*;

use std::collections::{HashMap, VecDeque};
use std::env;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::*;

use futures::future::{Future, TryFutureExt};
use futures::task::{Context, Poll};
use tokio::sync::Mutex;
use tokio::time::timeout;

use hyper::body;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Server, StatusCode};
use hyper::{Request, Response};

use failure::Fail;
use rand::Rng;
use uuid::Uuid;

use dotenv::dotenv;

pub mod error {
    use std::convert::From;
    pub use tokio::time::error::Elapsed as TimeoutError;

    #[derive(Debug, Fail)]
    pub enum Error {
        #[fail(display = "Timeout connecting to gateway")]
        TokioTimeoutError(),

        #[fail(display = "Hyper error: {}", _0)]
        HyperError(hyper::Error),
    }

    impl From<TimeoutError> for Error {
        fn from(_: TimeoutError) -> Self {
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
    type Output = Response<Body>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let waker = cx.waker();
        self.responses
            .try_lock()
            .and_then(|mut res| match res.remove(&self.request_id) {
                Some(response) => {
                    println!("Response found!");
                    Ok(Poll::Ready(response))
                }
                None => {
                    waker.clone().wake();
                    Ok(Poll::Pending)
                }
            })
            .or_else(|_| {
                print!(".");
                std::thread::sleep(std::time::Duration::from_millis(500));
                waker.clone().wake();
                Ok::<_, error::Error>(Poll::Pending)
            })
            .unwrap()
    }
}

#[derive(Clone)]
struct RequestProxy {
    secret: String,
    requests: Arc<Mutex<VecDeque<(Uuid, Request<::hyper::Body>)>>>,
    responses: Arc<Mutex<HashMap<Uuid, Response<::hyper::Body>>>>,
}

impl RequestProxy {
    async fn call(&self, req: Request<Body>) -> Result<Response<Body>, error::Error> {
        // Check if the Client read header is present, and if so, get the value.
        match req.headers().get("x-proxy-secret").map(|h| h.to_str()) {
            // If the value is not present, this is an external request to be forwarded to the client.
            // Push the request to the queue for the client.
            None => self.push_request(req).await,

            // If a secret key header was sent, and the key is correct,
            // then handle the authenticated client's request (forward a request, or receive a response).
            Some(Ok(key)) if key == self.secret => self.handle_proxy_client_request(req).await,

            // If the secret key header was sent, but the key is incorrect.
            Some(Ok(x)) => {
                println!("Incorrect secret key '{}'!", x);

                Ok(Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from("🐸 GET OUT".to_string()))
                    .unwrap())
            }

            // If the secret key header was sent, but we failed to read the value as a String.
            Some(Err(e)) => {
                eprintln!("Error decoding Secret Key: {}", e);

                Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from(
                        "🤢 Your request was bad and you should feel bad",
                    ))
                    .unwrap())
            }
        }
    }

    async fn handle_proxy_client_request(
        &self,
        request: Request<Body>,
    ) -> Result<Response<Body>, error::Error> {
        match request.method() {
            &Method::GET => self.pop_request().await,
            &Method::POST => self.push_response(request).await,
            _ => Ok(Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .header("content-type", "text/plain")
                .body(Body::from("😬 You suck at computers"))
                .unwrap()),
        }
    }

    async fn push_request(&self, req: Request<Body>) -> Result<Response<Body>, error::Error> {
        println!("{}", &req.uri());

        let request_id = Uuid::new_v4();

        {
            self.requests
                .lock()
                .await
                .push_back((request_id.clone(), req));
        }

        let await_response = ProxiedResponse {
            request_id,
            responses: self.responses.clone(),
        };

        let timeout_response: Response<Body> = Response::builder()
            .status(504)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from("😶 Timeout".to_string()))
            .unwrap();

        match timeout(Duration::from_secs(15), await_response).await {
            Ok(r) => Ok(r),
            Err(_) => {
                self.remove_request(request_id).await;
                Ok(timeout_response)
            }
        }
    }

    /// Remove a request from the request queue
    async fn remove_request(&self, request_id: Uuid) {
        // Find the index of the request
        let mut requests = self.requests.lock().await;

        let index = requests.iter().rposition(|req| req.0 == request_id);

        if let Some(i) = index {
            requests.remove(i);
        }
    }

    /// Pop a queued request, if any, and return the serialized request
    async fn pop_request(&self) -> Result<Response<Body>, error::Error> {
        let req = {
            self.requests
                .try_lock()
                .and_then(|mut r| Ok(r.pop_front()))
                .unwrap_or(None)
        };

        if req.is_none() {
            return Ok(Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Body::empty())
                .unwrap());
        }

        let (req_id, req) = req.expect("Failed to unwrap queued Request");
        let (parts, body) = req.into_parts();

        let bytes = body::to_bytes(body)
            .await
            .map_err(|e| error::Error::from(e))?
            .to_vec();

        let output = ProxiedRequest {
            id: req_id,
            method: parts.method.as_ref(),
            uri: RequestUri {
                path: parts.uri.path().to_string(),
                query: parts.uri.query().map(|q| q.to_string()),
                fragment: None, // it appears http::request::Uri does not support fragment
            },
            version: format!("{:?}", parts.version),
            headers: parts
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), Base64Bytes(value.as_bytes().to_vec())))
                .collect(),
            body: Base64Bytes(bytes),
        };

        Ok(Response::builder()
            .body(Body::from(
                serde_json::to_string(&output).expect("Failed to serialize to JSON"),
            ))
            .unwrap())
    }

    async fn push_response(&self, request: Request<Body>) -> Result<Response<Body>, error::Error> {
        println!("Received client POST response");

        let bytes = body::to_bytes(request.into_body())
            .await
            .map_err(|e| error::Error::from(e))?
            .to_vec();

        let body = String::from_utf8(bytes).expect("Failed to create string from bytes");

        let client_response = match serde_json::from_str::<ClientResponse>(&body) {
            Ok(r) => r,
            Err(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Body::from(
                        "🤢 Your request was bad and you should feel bad",
                    ))
                    .unwrap());
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
                    eprintln!("Somehow received a header with no name?");
                }
            }
        }

        let response = response;

        // TODO Check requests to verify the request ID is actually present

        {
            self.responses
                .lock()
                .await
                .insert(client_response.request_id, response);
        }

        // Update so that the ProxiedResponse future can continue

        Ok(Response::builder()
            .body(Body::from(
                client_response.request_id.to_hyphenated().to_string(),
            ))
            .unwrap())
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() {
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

    tokio::spawn(async move {
        let proxy = RequestProxy {
            secret: secret,
            requests: request_log.clone(),
            responses: response_log.clone(),
        };

        let make_svc = make_service_fn(|_| {
            let proxy_clone = proxy.clone();

            async move {
                Ok::<_, hyper::Error>(service_fn(move |request| {
                    let proxy_clone2 = proxy_clone.clone();
                    async move {
                        proxy_clone2
                            .call(request)
                            .map_err(move |e| e.compat())
                            .await
                    }
                }))
            }
        });

        let server = Server::bind(&listen_addr).serve(make_svc);

        println!("Listening on http://{}", listen_addr);

        // Run forever-ish...
        if let Err(err) = server.await {
            eprintln!("server error: {}", err);
        }
    })
    .await
    .unwrap();
}
