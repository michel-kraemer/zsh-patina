FROM rust:alpine AS chef 
RUN cargo install --locked cargo-chef 
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
ARG PROFILE=release
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --profile=${PROFILE} --recipe-path recipe.json
COPY . .
RUN cargo b --profile=${PROFILE}
RUN cp target/**/zsh-patina .

FROM alpine:latest
ARG PROFILE=release
RUN apk add --no-cache zsh expect
COPY --from=builder /build/zsh-patina /usr/local/bin/zsh-patina
