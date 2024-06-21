FROM rust:1.79

WORKDIR /usr/src/ssh_chat
COPY . .
RUN cargo install --path .

RUN useradd -ms /bin/bash ssh_chat
USER ssh_chat

CMD ["ssh_chat"]
