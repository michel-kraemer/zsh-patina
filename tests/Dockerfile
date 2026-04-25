FROM rust:alpine AS planner
RUN cargo install --locked cargo-chef
WORKDIR /build
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM rust:alpine AS builder
ARG PROFILE=debug
RUN cargo install --locked cargo-chef
WORKDIR /build
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --profile=${PROFILE} --recipe-path recipe.json
COPY . .
RUN cargo b --profile=${PROFILE}
RUN cp target/**/zsh-patina .

FROM alpine:latest
ARG PROFILE=debug
RUN apk add --no-cache zsh expect
COPY --from=builder /build/zsh-patina /usr/local/bin/zsh-patina
