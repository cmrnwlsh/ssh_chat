FROM rust:1.79 as builder

WORKDIR /usr/src/ssh_chat
COPY . .
RUN cargo install --path .

FROM debian:bookworm

RUN useradd -ms /bin/bash ssh_chat
RUN apt-get update && apt-get install -y && rm -rf /var/lib/apt/lists/*

USER ssh_chat
COPY --from=builder /usr/local/cargo/bin/ssh_chat /usr/local/bin/ssh_chat

CMD ["ssh_chat"]
