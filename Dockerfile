FROM rust:1.65-slim-buster AS builder

# This mainly follows the first article here, but all three are helpful or provided influence:

# https://blog.logrocket.com/packaging-a-rust-web-service-using-docker/ 
# https://dev.to/rogertorres/first-steps-with-docker-rust-30oi
# https://levelup.gitconnected.com/create-an-optimized-rust-alpine-docker-image-1940db638a6c 
# 
# The latter seems particularly nice if Alpine target was desired

# Step 1: Build *only* the dependencies to speed up future builds

WORKDIR /app/
RUN USER=root cargo new --bin request-proxy
WORKDIR /app/request-proxy
COPY ./Cargo.toml ./Cargo.toml

RUN apt-get update \
    && apt-get install -y pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Building just the dependencies, so we don't specify the "server" binary here
RUN cargo build --release
RUN rm src/*.rs

# Step 2: Now build the actual server

ADD . ./

RUN rm ./target/release/deps/request_proxy*
RUN cargo build --release --bin server

# Step 3: Copy the build into the image that will actually be run. 

FROM debian:bullseye-slim AS server
ARG APP=/usr/src/app 

RUN apt-get update \
    && apt-get install -y ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

ENV TZ=Etc/UTC \
    APP_USER=appuser

RUN groupadd $APP_USER \
    && useradd -g $APP_USER $APP_USER \ 
    && mkdir -p ${APP}

COPY --from=builder /app/request-proxy/target/release/server ${APP}/request-proxy

RUN chown -R $APP_USER:$APP_USER ${APP}

USER ${APP_USER}
WORKDIR ${APP}

ENV PORT=8080
EXPOSE ${PORT}

CMD ["./request-proxy"]