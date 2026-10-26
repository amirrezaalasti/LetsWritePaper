# LetsWritePaper sync relay: `paper serve` in a small container.
# Build from the repository root:
#   docker build -f deploy/relay.Dockerfile -t paper-relay .
#   docker run -p 8787:8787 -v paper-relay:/data -e PAPER_RELAY_SECRET=change-me paper-relay
FROM rust:1.89-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p paper-cli

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/paper /usr/local/bin/paper
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["paper", "serve", "--bind", "0.0.0.0", "--port", "8787", "--data", "/data"]
