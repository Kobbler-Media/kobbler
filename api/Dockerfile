FROM kobbler-tools:latest AS base

# koblibs is a sibling checkout referenced by path in api/Cargo.toml; compose
# passes it as the `koblibs` build context. It must land at /koblibs so that
# `../../koblibs/...` resolves from /app/api.
COPY --from=koblibs . /koblibs

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY .sqlx ./.sqlx
COPY api ./api
COPY kobo-proxy ./kobo-proxy
COPY libs ./libs

FROM base AS dev

ENV SQLX_OFFLINE=true
RUN cargo fetch

ENV CARGO_TARGET_DIR=/target
ENV RUSTFLAGS="--cfg tokio_unstable"
WORKDIR /app/api
CMD ["cargo", "watch", "-x", "run"]

FROM base AS builder
ENV SQLX_OFFLINE=true
ENV RUSTFLAGS="--cfg tokio_unstable"
# Cache mounts persist across builds (BuildKit). Target cache is ephemeral to
# this RUN, so copy the binary out before the step ends.
# The target mount's id keeps it apart from kobload's, which mounts the same path with a
# different toolchain and FFmpeg, so a shared dir would rebuild for whichever ran last.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,id=kobbler-target,target=/app/target \
    cargo build --release -p api && \
    cp /app/target/release/api /app/api-binary && \
    cp /app/target/release/chatbot_e2e /app/chatbot-e2e-binary

FROM kobbler-tools:latest AS prod
LABEL org.opencontainers.image.source="https://github.com/Kobbler-Media/kobbler"

WORKDIR /app
COPY --from=builder /app/api-binary /app/api
COPY --from=builder /app/chatbot-e2e-binary /app/chatbot_e2e
ENV BIND_ADDR=0.0.0.0:8080
CMD ["/app/api"]
