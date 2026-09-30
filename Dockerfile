# Acetate's server and CLI, for running in Docker (e.g. on a NAS).
#
#   scripts/build-image.sh            # build and push localhost:5000/acetate:latest
#
# Built without a database: the queries were checked when `.sqlx/` was made
# (`cargo sqlx prepare -- --all-targets`, after changing any query).

FROM rust:1-trixie AS build
WORKDIR /src
ENV SQLX_OFFLINE=true
COPY . .
# Cache mounts keep downloaded crates and compiled dependencies between builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
    && cp target/release/acetate /usr/local/bin/acetate

FROM debian:trixie-slim
# ffmpeg: music videos' facts (ffprobe) and thumbnails (a frame). Without it they're
# kept all the same, without those.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home acetate
COPY --from=build /usr/local/bin/acetate /usr/local/bin/acetate
COPY web /app/web
WORKDIR /app
# ACETATE_VOLUMES (where blobs go) and DATABASE_URL come from the compose file.
ENV ACETATE_WEB_DIR=/app/web \
    ACETATE_BIND=0.0.0.0:3000
USER acetate
EXPOSE 3000
ENTRYPOINT ["acetate"]
CMD ["serve"]
