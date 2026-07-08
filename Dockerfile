# Build stage — locales/ are embedded at compile time by fluent-templates.
FROM rust:1.96 AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY locales ./locales
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/target/release/trophy-bot /usr/local/bin/trophy-bot

# /app/data holds the SQLite database (when not using PostgreSQL);
# /app/images holds trophy images. Both are volumes in docker-compose.yml.
ENV DATABASE_URL=sqlite:///app/data/trophy.sqlite?mode=rwc

# The bot shuts down gracefully on SIGTERM (docker stop).
ENTRYPOINT ["trophy-bot"]
