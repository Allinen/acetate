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
# A release and the SHA-256 of its archives: what's downloaded has to match, so a
# replaced release file fails the build instead of going into the image. To update: the
# version, and `sha256sum` of each archive.
ARG MDBOOK_VERSION=0.5.4
ARG MDBOOK_SHA256_X86_64=3f28de05dafca9d0f2eab99c662116b0e37b89b1d96a08f8f430b9eeae958cd7
ARG MDBOOK_SHA256_AARCH64=753e5c5c363ee8a56972344dcf91466f005a51db84a7aeffe427ae3ef83d6d44
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && case "$(uname -m)" in \
         aarch64) target=aarch64-unknown-linux-musl; sum=$MDBOOK_SHA256_AARCH64 ;; \
         *) target=x86_64-unknown-linux-gnu; sum=$MDBOOK_SHA256_X86_64 ;; \
       esac \
    && curl -sSfL -o /tmp/mdbook.tar.gz "https://github.com/rust-lang/mdBook/releases/download/v$MDBOOK_VERSION/mdbook-v$MDBOOK_VERSION-$target.tar.gz" \
    && echo "$sum  /tmp/mdbook.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/mdbook.tar.gz -C /usr/local/bin \
    && rm /tmp/mdbook.tar.gz
COPY docs /src/docs
# (book.toml writes it to ../target/book.)
RUN cd /src && mdbook build docs

FROM debian:trixie-slim
# ffmpeg: music videos' facts (ffprobe) and thumbnails (a frame). Without it they're
# kept all the same, without those. python3 + yt-dlp: a YouTube video's metadata for the
# track editor's "import metadata from" (without, pasting its JSON still works). yt-dlp is
# a release of its own (Debian's goes stale, and YouTube changes), named here with its
# SHA-256 so the image never runs a file other than the one that was looked at. To update
# (when YouTube stops answering): the version and the checksum in the ADD line below, the
# checksum being the `yt-dlp` line of that release's SHA2-256SUMS.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg python3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --create-home acetate
ADD --chmod=755 --checksum=sha256:e51e69e89489f199530f13d32a4342da3ddb2cf34842f364603a066db05afbf3 \
    https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp /usr/local/bin/yt-dlp
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
