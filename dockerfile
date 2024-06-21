FROM rust:1.79 as builder

WORKDIR /usr/src/ssh_chat
COPY . .
RUN cargo install --path .

FROM debian:bullseye-slim

RUN useradd -ms /bin/bash ssh_chat
RUN apt-get update && apt-get install -y extra-runtime-dependencies && rm -rf /var/lib/apt/lists/*

USER ssh_chat
COPY --from=builder /usr/local/cargo/bin/myapp /usr/local/bin/ssh_chat

CMD ["ssh_chat"]
