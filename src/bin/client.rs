extern crate base64;
extern crate dotenv;
extern crate hyper;
extern crate request_proxy;
extern crate reqwest;
extern crate serde;
extern crate serde_json;
extern crate tokio;
extern crate uuid;

use request_proxy::types::*;

use dotenv::dotenv;
use std::str::FromStr;
use std::time::Duration;
use std::{env, thread, time};

use hyper::Version;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use reqwest::{Client, Method, StatusCode, Url};

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

async fn poll(client: &Client, server: &String, secret: &String, destination: &Url) {
    // Send poll for any new requests.
    let request = client
        .get(server)
        .header("x-proxy-secret", secret.as_str())
        .send();

    let response = match request.await {
        Ok(res) => res,
        Err(e) => {
            eprintln!("ERROR: {:?}\n", e);
            thread::sleep(time::Duration::from_millis(500));
            return;
        }
    };

    let response_status = response.status();

    // Read the server's response to a string.
    let content = match response.text().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to read response body! Error: {}", e);
            return;
        }
    };

    match response_status {
        // If the server just responded No Content then there's no requests at the moment.
        StatusCode::NO_CONTENT => {
            thread::sleep(time::Duration::from_millis(500));
            return;
        }
        // If the server responses unauthorized, then the secret key is probably wrong.
        StatusCode::UNAUTHORIZED => {
            println!("Error: Unauthorized! Is the $PROXY_SECRET correct?");
            println!("Server responded: {}", content);
            return;
        }
        // Everything else should be fine.
        _ => {}
    };

    // Try to decode the JSON
    let request: ProxiedRequest = match serde_json::from_str(&content) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            return;
        }
    };

    let method = Method::from_str(request.method).unwrap();

    let mut url = destination.clone();
    url.set_path(&request.uri.path);
    url.set_query(request.uri.query.as_deref());
    url.set_fragment(request.uri.fragment.as_deref());

    let mut headers = build_headers(&request);
    let body = String::from_utf8(request.body.0).unwrap();

    let mut host = destination.host_str().unwrap().to_owned();

    if let Some(port) = destination.port() {
        host.push_str(&format!(":{}", port));
    }

    let _ = headers.insert("host", HeaderValue::from_str(&host).unwrap());

    let mut full_url = url.path().to_string();
    url.query().and_then(|query| {
        full_url.push_str("?");
        full_url.push_str(query.as_ref());
        Some(())
    });
    url.fragment().and_then(|fragment| {
        full_url.push_str("#");
        full_url.push_str(fragment.as_ref());
        Some(())
    });

    // Print the first line; eg: "GET /some/resource HTTP/1.1".
    println!(
        "{} {} {:?}",
        &method,
        &full_url,
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

    match response.await {
        Ok(r) => {
            let r_status = r.status();
            let r_headers = r.headers().clone();

            let body = r
                .text()
                .await
                .expect("Failed to read response from server!");

            println!("{}", r_status);

            // Print all of the headers
            for (key, value) in r_headers.iter() {
                let error_message = "[undisplayable value]";
                let value_display = value.to_str().unwrap_or_else(|_| &error_message);
                println!("{}: {}", key, value_display);
            }

            println!("\n\n{}", body);

            // Build the response to send back to the server
            let proxied_response = ClientResponse {
                request_id: request.id,
                status: r_status.as_u16(),
                headers: ClientResponse::parse_header_map(&r_headers),
                body: Base64Bytes(body.into_bytes()),
            };

            match client
                .post(server)
                .header("x-proxy-secret", secret.as_str())
                .json(&proxied_response)
                .send()
                .await
            {
                Ok(_) => {
                    println!("\n=====================\nSuccessfully sent response to the server");
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
                .post(server)
                .header("x-proxy-secret", secret.as_str())
                .json(&proxied_response)
                .send()
                .await
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

#[tokio::main]
async fn main() {
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
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    tokio::spawn(async move {
        loop {
            poll(&client, &server, &secret, &destination).await;
        }
    })
    .await
    .unwrap()
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
