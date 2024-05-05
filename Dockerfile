FROM lukemathwalker/cargo-chef:latest as chef
WORKDIR /app

FROM chef AS planner
COPY ./Cargo.toml ./Cargo.lock ./
COPY ./gazzetta-common/Cargo.toml ./gazzetta-common/
COPY ./gazzetta-notify/Cargo.toml ./gazzetta-notify/
COPY ./gazzetta-server/Cargo.toml ./gazzetta-server/
COPY ./gazzetta-injest/Cargo.toml ./gazzetta-injest/
COPY ./gazzetta-common/src ./gazzetta-common/src
COPY ./gazzetta-notify/src ./gazzetta-notify/src
COPY ./gazzetta-server/src ./gazzetta-server/src
COPY ./gazzetta-injest/src ./gazzetta-injest/src
RUN cargo chef prepare

FROM chef AS builder
ARG COMPONENT_NAME
COPY --from=planner /app/recipe.json .
RUN cargo chef cook --release
COPY . .
RUN cargo build --release
RUN mv ./target/release/$COMPONENT_NAME ./app

FROM debian:stable-slim AS runtime
RUN apt update && apt install -y openssl ca-certificates && apt upgrade -y
RUN openssl s_client -connect www.gazzettaufficiale.it:443 -showcerts </dev/null 2>/dev/null | sed -e '/-----BEGIN/,/-----END/!d' | tee "/usr/local/share/ca-certificates/gazzetta.crt" >/dev/null
RUN update-ca-certificates
RUN useradd -ms /bin/bash app
USER app
WORKDIR /app
COPY --from=builder /app/app /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/app"]