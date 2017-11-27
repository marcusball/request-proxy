extern crate base64;
extern crate dotenv;
extern crate hyper;
extern crate request_proxy;
extern crate reqwest;
extern crate serde;
extern crate serde_json;
extern crate url;
extern crate uuid;

use request_proxy::types::*;

use std::{env, thread, time};
use std::io::Read;
use std::str::FromStr;
use dotenv::dotenv;

use hyper::{HttpVersion, Uri};
use reqwest::{Client, Method};
use reqwest::header::{Headers, Host, Raw};
use url::Url;

fn main() {
    dotenv().ok();

    // The hostname or IP of the server to which proxied requests were sent
    let server = env::var("REQUEST_PROXY_SERVER").expect("Missing REQUEST_PROXY_SERVER variable!");

    // Hostname or IP of the server to which to send proxied requests
    let destination = Url::from_str(&env::var("REQUEST_PROXY_HOST")
        .expect("Missing REQUEST_PROXY_HOST destination variable!"))
        .expect(
        "Failed to parse destination url!",
    );

    let destination_host = destination.host_str().unwrap();

    // Create an outbound request client
    let client = Client::new();

    loop {
        let response = reqwest::get(&server);

        if let Err(e) = response {
            println!("ERROR: {:?}\n", e);
            thread::sleep(time::Duration::from_millis(500));
            continue;
        }

        let mut response = response.unwrap();

        let mut content = String::new();
        response
            .read_to_string(&mut content)
            .expect("Failed to read response body!");

        if content.eq("NONE") {
            thread::sleep(time::Duration::from_millis(500));
            continue;
        }

        let request: ProxiedRequest = serde_json::from_str(&content).unwrap();

        let method = Method::from_str(request.method).unwrap();

        let mut url = destination.clone();
        url.set_path(request.uri);

        let mut headers = build_headers(&request);
        let body = String::from_utf8(request.body.0).unwrap();

        headers.set(Host::new(
            String::from(destination_host),
            destination.port(),
        ));

        println!(
            "{} {} {}\n{}\n{}",
            &method,
            Uri::from_str(request.uri).unwrap(),
            HttpVersion::from_str(&request.version).unwrap(),
            headers,
            &body
        );

        let response = client
            .request(method, url)
            .headers(headers)
            .body(body)
            .send();

        match response {
            Ok(mut r) => {
                let mut body = String::new();
                r.read_to_string(&mut body).ok();

                println!("{}\n{}\n{}", r.status(), r.headers(), body);

                // Build the response to send back to the server
                let proxied_response = ClientResponse {
                    request_id: request.id,
                    status: r.status().as_u16(),
                    headers: r.headers()
                        .iter()
                        .map(|header| {
                            (
                                header.name().to_owned(),
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
                    body: Base64Bytes(body.into_bytes()),
                };

                match client.post(&server).json(&proxied_response).send() {
                    Ok(_) => {},
                    Err(e) => {
                        println!("ERROR: Failed to send response to server! {:?}", e);
                    }
                };
            }
            Err(e) => {
                println!("{:?}", e);
            }
        }
        println!("\n-------------------------------------------\n");
    }
}

/// Builds a Headers object from the raw header values in the ProxiedRequest
fn build_headers(request: &ProxiedRequest) -> Headers {
    request
        .headers
        .iter()
        .fold(Headers::new(), |mut headers, &(k, ref v)| {
            let value_bytes: &[u8] = v.0.as_ref();
            headers.append_raw(String::from(k), Raw::from(value_bytes));
            headers
        })
}
