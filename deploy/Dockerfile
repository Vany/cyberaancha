# Static musl binary in an empty image (~15 MB). Built on the Mac via
# `make build-linux`; this file is shipped to n1 next to the binary — the box
# only ever runs `docker compose build` over a COPY, never compiles Rust.
FROM scratch
COPY aancha-server /aancha-server
ENTRYPOINT ["/aancha-server", "--config", "/app/aancha.toml"]
CMD ["serve"]
