FROM rust:1.89-bookworm AS builder

WORKDIR /app

COPY bot/Cargo.toml bot/Cargo.lock* ./bot/
RUN mkdir -p bot/src
COPY bot/src ./bot/src

WORKDIR /app/bot
RUN cargo build --release

FROM debian:bookworm-slim

WORKDIR /app

COPY --from=builder /app/bot/target/release/flasharb /app/flasharb
COPY bot/config.example.toml /app/config.example.toml

CMD ["/app/flasharb"]