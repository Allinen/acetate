# Acetate's server and CLI, for running in Docker (on a server, a NAS, a home lab...).
#
#   scripts/build-image.sh            # build and push to a registry (see the script)
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
# kept all the same, without those. python3 + yt-dlp: a YouTube video's metadata for the
# track editor's "import metadata from" (without, pasting its JSON still works). yt-dlp is
# its latest release (Debian's goes stale, and YouTube changes): rebuild to update it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg python3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home acetate
ADD --chmod=755 https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp /usr/local/bin/yt-dlp
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
