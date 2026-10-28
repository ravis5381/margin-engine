FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --bin margin-engine

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --create-home --uid 10001 margin
USER margin
COPY --from=build /src/target/release/margin-engine /usr/local/bin/margin-engine
COPY config/app.example.toml /etc/margin-engine/app.toml
COPY config/data_sources.example.toml /etc/margin-engine/data_sources.toml
ENV MARGIN_CONFIG=/etc/margin-engine/app.toml
ENV MARGIN_DATA_SOURCES=/etc/margin-engine/data_sources.toml
ENV MARGIN_LOG_FORMAT=json
EXPOSE 8080
ENTRYPOINT ["margin-engine"]
CMD ["serve"]
