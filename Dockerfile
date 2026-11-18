# Copies the prebuilt static binary; nothing compiles in here (CI or `just docker` builds it).
# Expects dist/linux-<arch>/zesquad for each target architecture.
FROM gcr.io/distroless/static:nonroot@sha256:aafe993665bf6200db38a02c11a9351e782a84b6a45e7f33b6695638c88e3931

ARG TARGETARCH
COPY --chmod=0755 dist/linux-${TARGETARCH}/zesquad /zesquad

ENV ZESQUAD_LISTEN=0.0.0.0:8080 \
    ZESQUAD_DATABASE=/data/zesquad.db \
    ZESQUAD_LOG_JSON=true
# The database directory, owned by distroless's nonroot user (65532). Docker initialises a new
# volume from it, so the app can create its database there; without it the volume is root's.
# Copied from the base image's empty nonroot home: no RUN, so no emulation per architecture.
COPY --from=gcr.io/distroless/static:nonroot@sha256:aafe993665bf6200db38a02c11a9351e782a84b6a45e7f33b6695638c88e3931 --chown=65532:65532 /home/nonroot /data
VOLUME /data
EXPOSE 8080
USER nonroot

# No curl in distroless: the binary checks itself.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["/zesquad", "healthcheck"]
ENTRYPOINT ["/zesquad"]
CMD ["serve"]
