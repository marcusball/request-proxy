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

use dotenv::dotenv;
use std::io::Read;
use std::str::FromStr;
use std::{env, thread, time};

use hyper::{Uri, Version};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, RedirectPolicy};
use url::Url;

/// Why the fuck doesn't the HTTP crate provide something like this already?
fn version_from_str(ver: &str) -> Version {
    match ver {
        "HTTP/0.9" => Version::HTTP_09,
        "HTTP/1.0" => Version::HTTP_10,
        "HTTP/1.1" => Version::HTTP_11,
        "HTTP/2.0" => Version::HTTP_2,
        v => panic!("Received invalid HTTP version '{}'!", v),
    }
}

fn main() {
    dotenv().ok();

    // The hostname or IP of the server to which proxied requests were sent
    let server = env::var("REQUEST_PROXY_SERVER").expect("Missing REQUEST_PROXY_SERVER variable!");

    // Hostname or IP of the server to which to send proxied requests
    let destination = Url::from_str(
        &env::var("REQUEST_PROXY_HOST").expect("Missing REQUEST_PROXY_HOST destination variable!"),
    )
    .expect("Failed to parse destination url!");

    let destination_host = destination.host_str().unwrap();

    loop {
        let client = Client::builder()
            .redirect(RedirectPolicy::none())
            .build()
            .unwrap();

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

        let request: ProxiedRequest = match serde_json::from_str(&content) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {}", e);
                continue;
            }
        };

        let method = Method::from_str(request.method).unwrap();

        let mut url = destination.clone();
        url.set_path(&request.uri);

        let mut headers = build_headers(&request);
        let body = String::from_utf8(request.body.0).unwrap();

        let _ = headers.insert("host", HeaderValue::from_str(destination.as_ref()).unwrap());

        println!(
            "{} {} {:?}\n{:?}\n{}",
            &method,
            request.uri,
            version_from_str(&request.version),
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

                println!("{}\n{:?}\n{}", r.status(), r.headers(), body);

                // Build the response to send back to the server
                let proxied_response = ClientResponse {
                    request_id: request.id,
                    status: r.status().as_u16(),
                    headers: r
                        .headers()
                        .iter()
                        .map(|(name, value)| {
                            (name.to_string(), Base64Bytes(value.as_bytes().to_vec()))
                        })
                        .collect(),
                    body: Base64Bytes(body.into_bytes()),
                };

                match client.post(&server).json(&proxied_response).send() {
                    Ok(_) => {
                        println!(
                            "\n=====================\nSuccessfully sent response to the server"
                        );
                    }
                    Err(e) => {
                        println!("ERROR: Failed to send response to server! {:?}", e);
                    }
                };
            }
            Err(e) => {
                println!("{:?}", e);

                // Build the response to send back to the server
                let proxied_response = ClientResponse {
                    request_id: request.id,
                    status: 500,
                    headers: Vec::new(),
                    body: Base64Bytes(Vec::new()),
                };

                match client.post(&server).json(&proxied_response).send() {
                    Ok(_) => {
                        println!("\n=====================\nSuccessfully notified server of error");
                    }
                    Err(e) => {
                        println!(
                            "ERROR: Failed to send error notification to server! {:?}",
                            e
                        );
                    }
                };
            }
        }
        println!("\n-------------------------------------------\n");
    }
}

/// Builds a Headers object from the raw header values in the ProxiedRequest
fn build_headers(request: &ProxiedRequest) -> HeaderMap {
    request
        .headers
        .iter()
        .fold(HeaderMap::new(), |mut headers, &(k, ref v)| {
            let value_bytes: &[u8] = v.0.as_ref();
            headers.append(
                HeaderName::from_str(k).unwrap(),
                HeaderValue::from_bytes(value_bytes).unwrap(),
            );
            headers
        })
}
