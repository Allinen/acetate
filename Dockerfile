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

# The user guide (docs/, an mdBook), served by the server at /docs.
FROM debian:trixie-slim AS book
ARG MDBOOK_VERSION=0.5.4
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && case "$(uname -m)" in aarch64) target=aarch64-unknown-linux-musl ;; *) target=x86_64-unknown-linux-gnu ;; esac \
    && curl -sSfL "https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VERSION/mdbook-v$MDBOOK_VERSION-$target.tar.gz" | tar -xz -C /usr/local/bin
COPY docs /src/docs
# (book.toml writes it to ../target/book.)
RUN cd /src && mdbook build docs

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
COPY --from=book /src/target/book /app/docs
# The programs of the extensions that come with Acetate (their data is in the binary).
COPY extensions /app/extensions
WORKDIR /app
# ACETATE_VOLUMES (where blobs go) and DATABASE_URL come from the compose file.
ENV ACETATE_WEB_DIR=/app/web \
    ACETATE_DOCS_DIR=/app/docs \
    ACETATE_BIND=0.0.0.0:3000
USER acetate
EXPOSE 3000
ENTRYPOINT ["acetate"]
CMD ["serve"]
