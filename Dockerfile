FROM rust:1.75-slim as builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY . .

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*


RUN useradd -m -u 1001 p2puser

WORKDIR /app

COPY --from=builder /app/target/release/node_eeb /app/node_eeb

RUN chown -R p2puser:p2puser /app

USER p2puser

EXPOSE 4001

ENTRYPOINT ["./node_eeb"]

CMD ["--port", "4001", "--name", "DockerNode"]