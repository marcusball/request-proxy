extern crate base64;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate reqwest;
extern crate dotenv;

use std::{env, thread, time};
use std::io::Read;
use dotenv::dotenv;

fn main() {
    dotenv().ok();

    // The hostname or IP of the server to which proxied requests were sent
    let server = env::var("REQUEST_PROXY_SERVER").expect("Missing REQUEST_PROXY_SERVER variable!");

    // Hostname or IP of the server to which to send proxied requests
    let host = env::var("REQUEST_PROXY_HOST").expect("Missing REQUEST_PROXY_HOST destination variable!");

    loop {
        let mut response = reqwest::get(&server).unwrap();

        let mut content = String::new();
        response.read_to_string(&mut content);

        println!("{}", content);

        thread::sleep(time::Duration::from_millis(500));
    }
}