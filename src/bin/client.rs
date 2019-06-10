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

use hyper::Version;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, RedirectPolicy, StatusCode};
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
    let server = env::var("PROXY_SERVER").expect("Missing $PROXY_SERVER variable!");

    // Hostname or IP of the server to which to send proxied requests
    let destination =
        Url::from_str(&env::var("PROXY_HOST").expect("Missing $PROXY_HOST destination variable!"))
            .expect("Failed to parse destination url!");

    // Shared secret key for reading requests/pushing responses
    let secret = env::var("PROXY_SECRET").expect("Missing $PROXY_SECRET variable!");

    let client = Client::builder()
        .redirect(RedirectPolicy::none())
        .build()
        .unwrap();

    loop {
        // Send poll for any new requests.
        let response = client
            .get(&server)
            .header("x-proxy-secret", secret.as_str())
            .send();

        // Unwrap the response, or pause if there was an error.
        let mut response = match response {
            Ok(res) => res,
            Err(e) => {
                eprintln!("ERROR: {:?}\n", e);
                thread::sleep(time::Duration::from_millis(500));
                continue;
            }
        };

        // Read the server's response to a string.
        let mut content = String::new();
        response
            .read_to_string(&mut content)
            .expect("Failed to read response body!");

        match response.status() {
            // If the server just responded No Content then there's no requests at the moment.
            StatusCode::NO_CONTENT => {
                thread::sleep(time::Duration::from_millis(500));
                continue;
            }
            // If the server responses unauthorized, then the secret key is probably wrong.
            StatusCode::UNAUTHORIZED => {
                println!("Error: Unauthorized! Is the $PROXY_SECRET correct?");
                println!("Server responded: {}", content);
                break;
            }
            // Everything else should be fine.
            _ => {}
        };

        // Try to decode the JSON
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

        let mut host = destination.host_str().unwrap().to_owned();

        if let Some(port) = destination.port() {
            host.push_str(&format!(":{}", port));
        }

        let _ = headers.insert("host", HeaderValue::from_str(&host).unwrap());

        // Print the first line; eg: "GET /some/resource HTTP/1.1".
        println!(
            "{} {} {:?}",
            &method,
            request.uri,
            version_from_str(&request.version)
        );

        // Print all of the headers
        for (key, value) in headers.iter() {
            let error_message = "[undisplayable value]";
            let value_display = value.to_str().unwrap_or_else(|_| &error_message);
            println!("{}: {}", key, value_display);
        }

        // Print the request body
        println!("\n\n{}", &body);

        let response = client
            .request(method, url)
            .headers(headers)
            .body(body)
            .send();

        println!("\n");

        match response {
            Ok(mut r) => {
                let mut body = String::new();
                r.read_to_string(&mut body).ok();

                println!("{}", r.status());

                // Print all of the headers
                for (key, value) in r.headers().iter() {
                    let error_message = "[undisplayable value]";
                    let value_display = value.to_str().unwrap_or_else(|_| &error_message);
                    println!("{}: {}", key, value_display);
                }

                println!("\n\n{}", body);

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

                match client
                    .post(&server)
                    .header("x-proxy-secret", secret.as_str())
                    .json(&proxied_response)
                    .send()
                {
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

                match client
                    .post(&server)
                    .header("x-proxy-secret", secret.as_str())
                    .json(&proxied_response)
                    .send()
                {
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
