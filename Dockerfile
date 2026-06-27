# syntax=docker/dockerfile:1

# ---- Stage 1: build the Vue frontend ----
FROM node:20-bookworm-slim AS frontend
WORKDIR /app/web-ui
COPY web-ui/package.json web-ui/package-lock.json* ./
RUN npm install --omit=optional
COPY web-ui/ ./
RUN npm run build

# ---- Stage 2: build the Rust binary ----
FROM rust:1-bookworm AS backend
WORKDIR /app
# Pre-fetch deps for better caching.
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release || true
# Real sources + prebuilt frontend.
COPY src/ ./src/
COPY --from=frontend /app/web-ui/dist ./web-ui/dist
RUN cargo build --release

# ---- Stage 3: minimal runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/MiniMask /usr/local/bin/minimask
# Default config is generated on first run if absent.
EXPOSE 8080 7443
ENTRYPOINT ["minimask", "server"]
