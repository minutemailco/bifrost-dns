FROM --platform=$BUILDPLATFORM rust:1.97-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
RUN cargo build --release

FROM scratch
COPY --from=builder /build/target/release/bifrost-dns /bifrost-dns
EXPOSE 53/udp 53/tcp 15353/tcp
ENTRYPOINT ["/bifrost-dns"]
