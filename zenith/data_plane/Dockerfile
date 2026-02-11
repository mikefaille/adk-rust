FROM lukemathwalker/cargo-chef:latest-rust-1.85 AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching layer!
COPY adk-rust /app/zenith/adk-rust
RUN mkdir -p zenith/data_plane && cd zenith/data_plane && cargo chef cook --release --recipe-path ../../recipe.json

# Build application
COPY . .
RUN cargo build --release
