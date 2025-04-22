FROM busybox AS package

COPY ./goreleaser-dist /tmp/goreleaser-dist

ARG TARGETARCH
RUN set -eux; \
    case "${TARGETARCH}" in \
      amd64) export RUST_TARGET_TRIPLE=x86_64-unknown-linux-musl;; \
      arm64) export RUST_TARGET_TRIPLE=aarch64-unknown-linux-musl;; \
      *) exit 1;; \
    esac; \
    ls -liah /tmp/goreleaser-dist/ \
    && cp /tmp/goreleaser-dist/publish-crates_${RUST_TARGET_TRIPLE}/publish-crates \
	/usr/bin/publish-crates

FROM scratch
COPY --from=package /usr/bin/publish-crates /usr/local/bin/publish-crates
ENTRYPOINT ["/usr/local/bin/publish-crates"]
