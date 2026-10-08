# The report collector (docs/collector.md). Build context: engine/
#   docker build -f engine/crates/emoticond-collector/Dockerfile -t emoticond-collector engine/
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p emoticond-collector

FROM debian:bookworm-slim
COPY --from=build /src/target/release/emoticond-collector /usr/local/bin/emoticond-collector
ENV EMOTICOND_COLLECTOR_DB=/data/reports.db EMOTICOND_COLLECTOR_ADDR=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
CMD ["emoticond-collector"]
