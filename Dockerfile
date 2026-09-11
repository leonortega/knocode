# Multi-stage: rust builder → distroless runtime
FROM rust:1.75-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/knocode /usr/local/bin/knocode
COPY --from=builder /app/target/release/knocode-daemon /usr/local/bin/knocode-daemon
EXPOSE 9527
ENTRYPOINT ["knocode-daemon"]
