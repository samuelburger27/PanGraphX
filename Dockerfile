FROM mcr.microsoft.com/devcontainers/rust:1

RUN apt-get update && apt-get install -y \
    build-essential \
    cmake \
    libjemalloc-dev \
    pkg-config \
    python3-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy

WORKDIR /pangraphx
COPY . .

RUN cargo build --workspace --features pangraphx-core/odgi --release

ENTRYPOINT ["./target/release/pangraphx-cli"]
