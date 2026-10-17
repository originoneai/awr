FROM rust:1.93.1-slim-trixie@sha256:c0a38f5662afdb298898da1d70b909af4bda4e0acff2dc52aea6360a9b9c6956

RUN apt-get update \
    && apt-get install --no-install-recommends -y python3 git pkg-config gcc-x86-64-linux-gnu \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-gnu

RUN apt-get update \
    && apt-get install --no-install-recommends -y libc6-dev-amd64-cross \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt clippy \
    && git config --system --add safe.directory /src

ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
ENV AR_x86_64_unknown_linux_gnu=x86_64-linux-gnu-ar
