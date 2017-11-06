extern crate request_proxy;
extern crate base64;
extern crate serde;
extern crate serde_json;
extern crate reqwest;
extern crate dotenv;
extern crate hyper; 

use request_proxy::types::*;

use std::{env, thread, time};
use std::io::Read;
use std::str::FromStr;
use dotenv::dotenv;

use hyper::{Method, Uri, HttpVersion};
use reqwest::header::{Headers, Raw};

fn main() {
    dotenv().ok();

    // The hostname or IP of the server to which proxied requests were sent
    let server = env::var("REQUEST_PROXY_SERVER").expect("Missing REQUEST_PROXY_SERVER variable!");

    // Hostname or IP of the server to which to send proxied requests
    let host = env::var("REQUEST_PROXY_HOST").expect("Missing REQUEST_PROXY_HOST destination variable!");

    loop {
        let mut response = reqwest::get(&server).unwrap();

        let mut content = String::new();
        response.read_to_string(&mut content).expect("Failed to read response body!");

        if content.eq("NONE") {
            thread::sleep(time::Duration::from_millis(500));
            continue;
        }

        let request: ProxiedRequest = serde_json::from_str(&content).unwrap();

        let headers = build_headers(&request);
        let body = String::from_utf8(request.body.0).unwrap();

        println!("{} {} {}\n{}\n{}", 
            Method::from_str(request.method).unwrap(), 
            Uri::from_str(request.uri).unwrap(),
            HttpVersion::from_str(&request.version).unwrap(),
            headers,
            &body
        );
    }
}

/// Builds a Headers object from the raw header values in the ProxiedRequest
fn build_headers(request: &ProxiedRequest) -> Headers {
    request.headers.iter().fold(Headers::new(), |mut headers, &(k, ref v)| {
        let value_bytes: &[u8] = v.0.as_ref();
        headers.append_raw(String::from(k), Raw::from(value_bytes));
        headers
    })
}