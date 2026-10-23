# syntax=docker/dockerfile:1
# Misthos desk image: `payroll serve` only (read-only; it cannot pay). Multi-stage on the pinned
# toolchain, run on debian slim as an unprivileged user. No seed, wallet or signing key in the image.
FROM rust:1.89-bookworm AS build
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} CARGO_TERM_COLOR=never
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
COPY ui ./ui
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bin payroll && cp target/release/payroll /payroll

FROM debian:bookworm-slim AS run
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 misthos
COPY --from=build /payroll /usr/local/bin/payroll
# The synthetic two-employee roster from the README's 60-second command (made by e2e/make-fixtures.py).
COPY e2e/fixtures/roster.csv /app/roster.csv
ENV PAYROLL_ROSTER_CSV=/app/roster.csv
USER misthos
WORKDIR /app
EXPOSE 8787
CMD ["payroll", "serve"]
