# Abe — multi-model LLM debate. Container runs the web UI + JSON API.
# NOTE: CLI providers (codex/claude/opencode) are NOT in this image; use HTTP
# providers (openai / anthropic / openai-compatible) for Dockerized debates.

# --- build ---
# Pin to bookworm so the builder's glibc matches the bookworm-slim runtime below
# (rust:1-slim tracks newer Debian, whose glibc the runtime lacks).
FROM rust:1-slim-bookworm AS build
WORKDIR /app
# ponytail: straight copy + build — no dummy-main dep-cache trick until builds are slow enough to need it.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# --- runtime ---
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/abe /usr/local/bin/abe
EXPOSE 8080
# 0.0.0.0 so the mapped port is reachable from the host. Mount your config at
# /config.yaml:  docker run -p 8080:8080 -v ./abe.yaml:/config.yaml ghcr.io/yonk-labs/abe
ENTRYPOINT ["abe"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8080", "--config", "/config.yaml"]
