# Request Proxy

## What is this? 

This is a tool to help expose local, internal services without needing to configure
NAT or firewall rules, etc. This is essentially a basic version of [ngrok](https://ngrok.com/). 

All you need to do is set up the server binary on a cloud provider, or external server, 
then run the client from a local machien. 

## Configuration

This uses environment variables for configuration. See the `.env.example` file,
copy it into your own `.env` file. 

```
## Shared Secret Key
PROXY_SECRET=bSAgJEuVX0y05R3R6clf5rE9cS2xYmbDyD0cuwcs

## Server Variables
# The address on which to bind. Eg: 0.0.0.0. 
LISTEN_IP=127.0.0.1
# The Port on which to listen. 
PORT=3000

## Client Variables
#
# This is the URL of the externally visible Server. 
PROXY_SERVER=https://some.external.service.test:3000/
# This is the desired internal "Host" to which requests should be sent. 
PROXY_HOST=https://some.internal.service.test/
```

Generate a new, random `PROXY_SECRET` which will be shared between your client and server. 

On the Client, `PROXY_SERVER` is the address of the externally visible server. This is also the address
which will be used for accessing the internal service. 

`PROXY_HOST` is the address of the internal service to which requests will be forwarded. 

## Usage 

Build both:

```
cargo build
```

Running the server: 

```
cargo run --bin server
```

Running the client:

```
cargo run --bin client
```