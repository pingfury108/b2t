# ---------- 前端构建 ----------
FROM node:24-alpine AS frontend
RUN corepack enable
WORKDIR /build
COPY . .
RUN pnpm --dir frontend install --frozen-lockfile \
    && pnpm --dir frontend build

# ---------- 后端构建 ----------
FROM rust:1.92-bookworm AS backend
WORKDIR /build
COPY . .
COPY --from=frontend /build/frontend/dist ./frontend/dist
ENV B2T_SKIP_FRONTEND_BUILD=1
RUN cargo build --release

# ---------- 运行时 ----------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ffmpeg python3 python3-pip ca-certificates \
    && pip3 install --break-system-packages --no-cache-dir edge-tts \
    && apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=backend /build/target/release/b2t /usr/local/bin/b2t

ENV B2T_HOST=0.0.0.0 \
    PORT=3000 \
    B2T_DATA=/data
VOLUME ["/data"]
EXPOSE 3000

CMD ["b2t"]
