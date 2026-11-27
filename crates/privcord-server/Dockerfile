FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p privcord-server

FROM debian:bookworm-slim
COPY --from=build /src/target/release/privcord-server /usr/local/bin/privcord-server
VOLUME /data
WORKDIR /data
ENTRYPOINT ["privcord-server"]
CMD ["serve"]
