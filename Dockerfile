FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked || cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates         && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/evgl-api /usr/local/bin/evgl-api
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["evgl-api"]
